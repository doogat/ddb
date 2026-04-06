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

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::junction_table_ddl;
use crate::indexer::Index;
use crate::parser;
use crate::traits::DoogatStore;
use crate::types::{
    ColumnDef, DoogatId, DoogatMeta, InlineField, Link, ParsedDoogat, TableSchema, Value, Zone,
};

/// Strip surrounding double-quotes from a SQL identifier.
/// sqlparser preserves quotes in `to_string()` for identifiers like `"meeting-minutes"`.
fn unquote_identifier(s: &str) -> String {
    s.trim_matches('"').to_lowercase()
}

/// Extract the primary table name from a statement's FROM clause.
/// Returns the first plain table relation found, or None for subqueries/joins/CTEs.
fn extract_from_table(stmt: &Statement) -> Option<String> {
    let query = match stmt {
        Statement::Query(q) => q,
        _ => return None,
    };
    let select = match query.body.as_ref() {
        SetExpr::Select(s) => s,
        _ => return None,
    };
    if select.from.len() != 1 {
        return None;
    }
    if !select.from[0].joins.is_empty() {
        return None;
    }
    let relation = &select.from[0].relation;
    match relation {
        sqlparser::ast::TableFactor::Table { name, .. } => {
            Some(unquote_identifier(&name.to_string()))
        }
        _ => None,
    }
}

#[derive(Debug)]
pub enum SqlResult {
    Rows {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        /// Optional data types for each column (e.g. "BOOLEAN", "INTEGER", "TEXT").
        /// Present when the query targets a materialized type table with a typedef.
        column_types: Option<Vec<String>>,
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
    pub doogat_id: String,
}

#[derive(Default)]
pub struct TransactionBuffer {
    pub writes: Vec<PendingWrite>,
    pub deletes: Vec<PendingDelete>,
}

pub struct SqlEngine<'a> {
    index: &'a Index,
    repo: &'a dyn DoogatStore,
    txn: Option<TransactionBuffer>,
}

/// Reserved table names that cannot be used for CREATE TABLE.
fn is_reserved_table(name: &str) -> bool {
    name == "doogats" || name.starts_with("_ddb_") || name.starts_with("sqlite_")
}

impl Drop for SqlEngine<'_> {
    fn drop(&mut self) {
        if self.txn.take().is_some() {
            if let Err(e) = self.index.conn.execute("ROLLBACK TO ddb_txn", []) {
                tracing::warn!(error = %e, "sql_engine drop: rollback failed");
            }
            if let Err(e) = self.index.conn.execute("RELEASE ddb_txn", []) {
                tracing::warn!(error = %e, "sql_engine drop: release failed");
            }
        }
    }
}

impl<'a> SqlEngine<'a> {
    pub fn new(index: &'a Index, repo: &'a dyn DoogatStore) -> Self {
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

    /// Generate a unique DoogatId, waiting if same-second collision detected.
    fn unique_id(&mut self) -> Result<DoogatId> {
        self.unique_ids(1).map(|mut v| v.remove(0))
    }

    /// Generate `count` unique DoogatIds without sleeping between them.
    ///
    /// Gets a base timestamp via `generate_unique_id`, then increments by 1
    /// second for each subsequent ID, skipping any that already exist in the
    /// index.
    fn unique_ids(&mut self, count: usize) -> Result<Vec<DoogatId>> {
        use chrono::NaiveDateTime;

        let mut ids = Vec::with_capacity(count);
        let first = parser::generate_unique_id(|candidate| {
            self.index
                .conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM doogats WHERE id = ?1",
                    params![candidate],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        });

        let mut ts = NaiveDateTime::parse_from_str(&first.0, "%Y%m%d%H%M%S").map_err(|e| {
            DoogatError::SqlEngine(format!("failed to parse generated id timestamp: {e}"))
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
                        "SELECT COUNT(*) > 0 FROM doogats WHERE id = ?1",
                        params![&candidate],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if !exists {
                    ids.push(DoogatId(candidate));
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
            return Err(DoogatError::SqlEngine(
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
            .map_err(|e| DoogatError::SqlEngine(format!("parse: {e}")))?;

        if statements.is_empty() {
            return Err(DoogatError::SqlEngine("no SQL statements".into()));
        }

        // Check if batch contains an explicit BEGIN — if so, let user manage txn
        let has_explicit_txn = statements.iter().any(|s| {
            matches!(
                s,
                Statement::StartTransaction { .. } | Statement::Commit { .. }
            )
        });

        // Wrap in implicit transaction when no explicit txn and no txn already active
        let implicit_txn = !has_explicit_txn && self.txn.is_none() && statements.len() > 1;
        if implicit_txn {
            self.handle_begin()?;
        }

        let mut results = Vec::with_capacity(statements.len());
        for stmt in &statements {
            match self.execute_statement(stmt) {
                Ok(r) => results.push(r),
                Err(e) => {
                    if implicit_txn {
                        let _ = self.handle_rollback();
                    }
                    return Err(e);
                }
            }
        }

        if implicit_txn {
            self.handle_commit()?;
        }

        Ok(results)
    }

    fn execute_statement(&mut self, stmt: &Statement) -> Result<SqlResult> {
        match stmt {
            Statement::CreateTable(ct) => self.handle_create_table(ct),
            Statement::Insert(ins) => self.handle_insert(ins),
            Statement::Update { table, assignments, from, selection, .. } => {
                if from.is_some() {
                    return Err(DoogatError::SqlEngine(
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
                Err(DoogatError::SqlEngine(
                    "DROP INDEX not supported: indexes are managed automatically and rebuilt on reindex".into(),
                ))
            }
            Statement::Drop { object_type: ObjectType::View, .. } => {
                Err(DoogatError::SqlEngine(
                    "DROP VIEW not supported: views cannot be created".into(),
                ))
            }
            Statement::Drop { object_type, if_exists, names, cascade, .. } => {
                self.handle_drop(object_type, *if_exists, names, *cascade)
            }
            Statement::CreateIndex(_) => {
                Err(DoogatError::SqlEngine(
                    "CREATE INDEX not supported: indexes on the materialized cache are rebuilt from doogat data on reindex".into(),
                ))
            }
            Statement::CreateView { .. } => {
                Err(DoogatError::SqlEngine(
                    "CREATE VIEW not supported: views are not stored as doogats and are lost on reindex".into(),
                ))
            }
            Statement::CreateVirtualTable { .. } => {
                Err(DoogatError::SqlEngine(
                    "CREATE VIRTUAL TABLE not supported: virtual tables have no doogat representation".into(),
                ))
            }
            Statement::CreateTrigger { .. } => {
                Err(DoogatError::SqlEngine(
                    "CREATE TRIGGER not supported: triggers fire on cache mutations, not git commits".into(),
                ))
            }
            Statement::AlterIndex { .. } => {
                Err(DoogatError::SqlEngine(
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
                let (rows, column_types) = self.coerce_boolean_columns(stmt, &columns, rows);
                Ok(SqlResult::Rows { columns, rows, column_types })
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
                    return Err(DoogatError::NotFound(format!(
                        "deleted in transaction: {path}"
                    )));
                }
            }
        }
        self.repo.read_file(path)
    }

    fn handle_begin(&mut self) -> Result<SqlResult> {
        if self.txn.is_some() {
            return Err(DoogatError::SqlEngine("transaction already active".into()));
        }
        self.index
            .conn
            .execute("SAVEPOINT ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("savepoint: {e}")))?;
        self.txn = Some(TransactionBuffer::default());
        Ok(SqlResult::Ok("BEGIN".into()))
    }

    fn handle_commit(&mut self) -> Result<SqlResult> {
        let buf = self
            .txn
            .as_ref()
            .ok_or_else(|| DoogatError::SqlEngine("no active transaction".into()))?;

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
            .execute("RELEASE ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("release: {e}")))?;
        // Clear txn only after both git commit and RELEASE succeed
        self.txn.take();
        Ok(SqlResult::Ok("COMMIT".into()))
    }

    fn handle_rollback(&mut self) -> Result<SqlResult> {
        if self.txn.is_none() {
            return Err(DoogatError::SqlEngine("no active transaction".into()));
        }
        self.index
            .conn
            .execute("ROLLBACK TO ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("rollback: {e}")))?;
        self.index
            .conn
            .execute("RELEASE ddb_txn", [])
            .map_err(|e| DoogatError::SqlEngine(format!("release: {e}")))?;
        // Only clear txn after SQLite ops succeed — Drop still cleans up on failure
        self.txn.take();
        Ok(SqlResult::Ok("ROLLBACK".into()))
    }

    fn handle_create_table(&mut self, ct: &sqlparser::ast::CreateTable) -> Result<SqlResult> {
        let table_name = unquote_identifier(&ct.name.to_string());

        if is_reserved_table(&table_name) {
            return Err(DoogatError::SqlEngine(format!(
                "reserved table name: {table_name}"
            )));
        }

        // Check if typedef already exists
        let existing: Option<String> = self
            .index
            .conn
            .query_row(
                "SELECT id FROM doogats WHERE type = '_typedef' AND title = ?1",
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
            return Err(DoogatError::SqlEngine(format!(
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
            unique_together: None,
        };

        // Build and commit typedef doogat
        let id = self.unique_id()?;
        let schema_doogat = build_typedef_doogat(&id, &schema);
        let content = parser::serialize(&schema_doogat);
        let path = format!("ddb/_typedef/{}.md", id.0);
        self.repo
            .commit_file(&path, &content, &format!("create table {table_name}"))?;

        // Index the typedef doogat
        let parsed = parser::parse(&content, &path)?;
        self.index.index_doogat(&parsed)?;

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
            let default_value = extract_default(&col.options)?;
            if let Some(ref dv) = default_value {
                if (dv == "NEXT" || dv.starts_with("NEXT("))
                    && !data_type.eq_ignore_ascii_case("integer")
                {
                    return Err(DoogatError::SqlEngine(format!(
                        "DEFAULT NEXT is only valid on INTEGER columns, not {data_type}"
                    )));
                }
            }
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

        // Validate NEXT(col) partition columns exist in the table
        let col_names: Vec<&str> = out.iter().map(|c| c.name.as_str()).collect();
        for col in &out {
            if let Some(ref dv) = col.default_value {
                if dv.starts_with("NEXT(") && dv.ends_with(')') {
                    let partition_col = &dv[5..dv.len() - 1];
                    if !col_names.contains(&partition_col) {
                        return Err(DoogatError::SqlEngine(format!(
                            "DEFAULT NEXT({partition_col}): column '{partition_col}' not found in table"
                        )));
                    }
                }
            }
        }

        Ok(out)
    }

    fn create_materialized_table(&mut self, schema: &TableSchema) -> Result<()> {
        let mut col_defs = vec![
            "id TEXT PRIMARY KEY".to_string(),
            "title TEXT".to_string(),
            "date TEXT".to_string(),
            "updated_at TEXT".to_string(),
        ];
        for col in &schema.columns {
            if is_core_column(&col.name) {
                continue;
            }
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
                self.index
                    .conn
                    .execute(&junction_table_ddl(&schema.table_name, &col.name), [])?;
            }
        }

        Ok(())
    }

    fn handle_insert(&mut self, ins: &sqlparser::ast::Insert) -> Result<SqlResult> {
        // Reject REPLACE/UPSERT variants that bypass git
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
        let on_conflict_ignore =
            if let Some(ref on_conflict) = ins.on {
                use sqlparser::ast::{OnConflictAction, OnInsert};
                match on_conflict {
                    OnInsert::OnConflict(oc) => match oc.action {
                        OnConflictAction::DoNothing => true,
                        _ => return Err(DoogatError::SqlEngine(
                            "ON CONFLICT DO UPDATE is not supported; only DO NOTHING is allowed"
                                .into(),
                        )),
                    },
                    _ => {
                        return Err(DoogatError::SqlEngine(
                            "INSERT OR REPLACE/UPSERT not supported: bypasses git storage".into(),
                        ))
                    }
                }
            } else {
                false
            };

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
                        rows.push(eval_values(&self.index.conn, row)?);
                    }
                    rows
                }
                _ => {
                    return Err(DoogatError::SqlEngine(
                        "only VALUES clause supported".into(),
                    ))
                }
            },
            None => return Err(DoogatError::SqlEngine("missing VALUES clause".into())),
        };

        // When ON CONFLICT DO NOTHING, filter out rows that match an existing unique_together
        // constraint and capture the existing row IDs so we can return them alongside new IDs.
        let mut on_conflict_existing: Vec<Option<String>> = Vec::new();
        let rows: Vec<Vec<String>> = if on_conflict_ignore {
            if let Some(ref constraints) = schema.unique_together {
                on_conflict_existing = vec![None; rows.len()];
                let mut filtered = Vec::with_capacity(rows.len());
                'row: for (row_idx, row_values) in rows.into_iter().enumerate() {
                    for constraint_cols in constraints {
                        // Build WHERE clause for this constraint group
                        let where_clause: String = constraint_cols
                            .iter()
                            .map(|c| format!("\"{}\" = ?", c))
                            .collect::<Vec<_>>()
                            .join(" AND ");
                        let sql = format!(
                            "SELECT id FROM \"{}\" WHERE {}",
                            schema.table_name, where_clause
                        );
                        // Collect bind values for this constraint
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
                        if bind_vals.len() == constraint_cols.len() {
                            let existing_id: Option<String> = self
                                .index
                                .conn
                                .query_row(&sql, rusqlite::params_from_iter(bind_vals), |row| {
                                    row.get(0)
                                })
                                .ok();
                            if let Some(id) = existing_id {
                                on_conflict_existing[row_idx] = Some(id);
                                continue 'row;
                            }
                        }
                    }
                    filtered.push(row_values);
                }
                filtered
            } else {
                rows
            }
        } else {
            rows
        };

        // Generate all IDs upfront
        let ids = self.unique_ids(rows.len())?;

        // Collect which referenced types use folder storage (for path-qualified wikilinks)
        let ref_folder_types = self.ref_folder_types(&schema);

        let mut created_ids = Vec::with_capacity(rows.len());
        let mut files: Vec<(String, String)> = Vec::with_capacity(rows.len());

        // Pre-compute NEXT counters for auto-increment columns
        let mut next_counters: BTreeMap<String, i64> = BTreeMap::new();
        for col_def in &schema.columns {
            if let Some(ref dv) = col_def.default_value {
                if dv == "NEXT" {
                    let max_val: i64 = self
                        .index
                        .conn
                        .query_row(
                            &format!(
                                "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\"",
                                col_def.name, schema.table_name
                            ),
                            [],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    next_counters.insert(col_def.name.clone(), max_val);
                }
            }
        }

        for (row_values, id) in rows.iter().zip(ids.into_iter()) {
            if col_names.len() != row_values.len() {
                return Err(DoogatError::SqlEngine(
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
                        if default == "NEXT" {
                            let counter = next_counters.get_mut(&col_def.name).unwrap();
                            *counter += 1;
                            col_values.insert(col_def.name.clone(), counter.to_string());
                        } else if default.starts_with("NEXT(") && default.ends_with(')') {
                            let partition_col = &default[5..default.len() - 1];
                            let partition_val =
                                col_values.get(partition_col).cloned().unwrap_or_default();
                            let max_val: i64 = self
                                .index
                                .conn
                                .query_row(
                                    &format!(
                                        "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\" WHERE \"{}\" = ?1",
                                        col_def.name, schema.table_name, partition_col
                                    ),
                                    params![partition_val],
                                    |row| row.get(0),
                                )
                                .unwrap_or(0);
                            col_values.insert(col_def.name.clone(), (max_val + 1).to_string());
                        } else {
                            col_values.insert(col_def.name.clone(), default.clone());
                        }
                    }
                }
            }

            // Validate allowed_values constraints
            for col_def in &schema.columns {
                if let Some(ref allowed) = col_def.allowed_values {
                    if let Some(val) = col_values.get(&col_def.name) {
                        if !val.is_empty() && !allowed.contains(val) {
                            return Err(DoogatError::Validation(format!(
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
                                    "SELECT COUNT(*) > 0 FROM doogats WHERE id = ?1",
                                    params![ref_id],
                                    |row| row.get(0),
                                )
                                .unwrap_or(false);
                            if !exists {
                                return Err(DoogatError::SqlEngine(format!(
                                    "referenced doogat not found: {}",
                                    ref_id
                                )));
                            }
                        }
                    }
                }
            }

            // Build doogat
            let doogat = build_data_doogat(&id, &schema, &col_values, &ref_folder_types);
            let content = parser::serialize(&doogat);
            let path = if table_name == "doogats" {
                format!("ddb/{}.md", id.0)
            } else {
                crate::git_ops::doogat_path(&id.0, Some(&table_name), schema.folder)
            };

            // Index the doogat
            let parsed = parser::parse(&content, &path)?;
            self.index.index_doogat(&parsed)?;

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

        if on_conflict_ignore && !on_conflict_existing.is_empty() {
            // Merge existing IDs (for skipped duplicates) with newly created IDs
            let mut created_iter = created_ids.into_iter();
            let all_ids: Vec<String> = on_conflict_existing
                .into_iter()
                .map(|slot| match slot {
                    Some(id) => id,
                    None => created_iter.next().unwrap_or_default(),
                })
                .collect();
            Ok(SqlResult::Ok(all_ids.join(",")))
        } else {
            Ok(SqlResult::Ok(created_ids.join(",")))
        }
    }

    fn handle_update(
        &mut self,
        table: &sqlparser::ast::TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
        selection: &Option<Expr>,
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&table.relation.to_string());
        let schema = self.load_schema(&table_name)?;

        // Build assignment map: literals evaluated now, complex expressions deferred
        let mut updates: BTreeMap<String, String> = BTreeMap::new();
        let mut deferred: Vec<(String, String)> = Vec::new(); // (col_name, sql_text)
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
                let val = expr_to_string(&assignment.value)?;
                updates.insert(col_name, val);
            } else {
                let sql = value_to_sql(&assignment.value)?;
                deferred.push((col_name, sql));
            }
        }

        let validate_allowed_values =
            |schema: &TableSchema, updates: &BTreeMap<String, String>| -> Result<()> {
                for col_def in &schema.columns {
                    if let Some(ref allowed) = col_def.allowed_values {
                        if let Some(val) = updates.get(&col_def.name) {
                            if !val.is_empty() && !allowed.contains(val) {
                                return Err(DoogatError::Validation(format!(
                                    "column '{}': value '{}' not in allowed values {:?}",
                                    col_def.name, val, allowed
                                )));
                            }
                        }
                    }
                }
                Ok(())
            };

        // Validate literal assignments early (fail fast before touching rows)
        validate_allowed_values(&schema, &updates)?;

        let eval_deferred = |conn: &rusqlite::Connection,
                             deferred: &[(String, String)],
                             table_name: &str,
                             doogat_id: &str,
                             updates: &mut BTreeMap<String, String>|
         -> Result<()> {
            for (col, sql) in deferred {
                let eval_sql = format!("SELECT {sql} FROM \"{table_name}\" WHERE id = ?1");
                let result: rusqlite::types::Value = conn
                    .query_row(&eval_sql, rusqlite::params![doogat_id], |row| row.get(0))
                    .map_err(|e| DoogatError::SqlEngine(format!("expression eval failed: {e}")))?;
                updates.insert(col.clone(), sqlite_value_to_string(result)?);
            }
            Ok(())
        };

        // Fast path: single-row WHERE id = '...'
        if let Ok(doogat_id) = extract_where_id(selection) {
            if !deferred.is_empty() {
                eval_deferred(
                    &self.index.conn,
                    &deferred,
                    &table_name,
                    &doogat_id,
                    &mut updates,
                )?;
                // Re-validate after deferred expressions are resolved
                validate_allowed_values(&schema, &updates)?;
            }
            let path = self.index.resolve_path(&doogat_id)?;
            let content = self.read_content(&path)?;
            let mut parsed = parser::parse(&content, &path)?;
            apply_updates_to_doogat(&mut parsed, &schema, &updates);
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
            self.index.index_doogat(&reparsed)?;
            self.update_materialized_row(&schema, &doogat_id, &updates)?;
            return Ok(SqlResult::Affected(1));
        }

        // Bulk path: resolve matching rows via SQLite
        let matches = self.resolve_matching_ids(&table_name, selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        let mut files: Vec<(String, String)> = Vec::with_capacity(matches.len());
        let mut per_row_updates: Vec<BTreeMap<String, String>> = Vec::with_capacity(matches.len());
        for (id, path) in &matches {
            let mut row_updates = updates.clone();
            if !deferred.is_empty() {
                eval_deferred(
                    &self.index.conn,
                    &deferred,
                    &table_name,
                    id,
                    &mut row_updates,
                )?;
                validate_allowed_values(&schema, &row_updates)?;
            }
            let content = self.read_content(path)?;
            let mut parsed = parser::parse(&content, path)?;
            apply_updates_to_doogat(&mut parsed, &schema, &row_updates);
            files.push((path.clone(), parser::serialize(&parsed)));
            per_row_updates.push(row_updates);
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
        for ((id, path), row_updates) in matches.iter().zip(per_row_updates.iter()) {
            let content = self.read_content(path)?;
            let reparsed = parser::parse(&content, path)?;
            self.index.index_doogat(&reparsed)?;
            self.update_materialized_row(&schema, id, row_updates)?;
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
            .ok_or_else(|| DoogatError::SqlEngine("missing table in DELETE".into()))?;

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
        if let Ok(doogat_id) = extract_where_id(&del.selection) {
            let path = self.index.resolve_path(&doogat_id)?;
            self.index.remove_doogat(&doogat_id)?;
            self.index.conn.execute(
                &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
                params![doogat_id],
            )?;
            self.cascade_junction_cleanup(&table_name, &doogat_id)?;
            let ref_edits = self.cascade_remove_dangling_references(&doogat_id, &path)?;
            if let Some(ref mut buf) = self.txn {
                buf.deletes.push(PendingDelete {
                    path: path.clone(),
                    doogat_id: doogat_id.clone(),
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
            return Ok(SqlResult::Affected(1));
        }

        // Bulk path: resolve matching rows via SQLite
        let matches = self.resolve_matching_ids(&table_name, &del.selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        let mut all_ref_edits: Vec<PendingWrite> = Vec::new();
        for (id, path) in &matches {
            self.index.remove_doogat(id)?;
            self.index.conn.execute(
                &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
                params![id],
            )?;
            self.cascade_junction_cleanup(&table_name, id)?;
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
                        return Err(DoogatError::SqlEngine(format!(
                            "column already exists: {col_name}"
                        )));
                    }
                    let dt = data_type_to_string(&column_def.data_type);
                    let refs = extract_references(&column_def.options);
                    let default_value = extract_default(&column_def.options)?;
                    // Validate NEXT defaults on ALTER TABLE ADD COLUMN
                    if let Some(ref dv) = default_value {
                        if (dv == "NEXT" || dv.starts_with("NEXT("))
                            && !dt.eq_ignore_ascii_case("integer")
                        {
                            return Err(DoogatError::SqlEngine(format!(
                                "DEFAULT NEXT is only valid on INTEGER columns, not {dt}"
                            )));
                        }
                        if dv.starts_with("NEXT(") && dv.ends_with(')') {
                            let partition_col = &dv[5..dv.len() - 1];
                            let all_cols: Vec<&str> =
                                schema.columns.iter().map(|c| c.name.as_str()).collect();
                            if !all_cols.contains(&partition_col) && partition_col != col_name {
                                return Err(DoogatError::SqlEngine(format!(
                                    "DEFAULT NEXT({partition_col}): column '{partition_col}' not found in table"
                                )));
                            }
                        }
                    }
                    let zone = if refs.is_some() {
                        Some(Zone::Reference)
                    } else if is_numeric_type(&dt) || is_short_string_type(&column_def.data_type) {
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
                        default_value,
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
                            return Err(DoogatError::SqlEngine(format!(
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
                    return Err(DoogatError::SqlEngine(format!(
                        "unsupported ALTER TABLE operation: {other}"
                    )));
                }
            }
        }

        self.update_typedef(&table_name, &schema)?;
        self.index.rematerialize_type(&table_name, self.repo)?;

        Ok(SqlResult::Ok(format!("table {table_name} altered")))
    }

    /// Serialize a modified TableSchema back to its typedef doogat, commit to Git, re-index.
    fn update_typedef(&mut self, table_name: &str, schema: &TableSchema) -> Result<()> {
        let (typedef_id, typedef_path) = self.load_typedef_location(table_name)?;
        let id = DoogatId(typedef_id);
        let schema_doogat = build_typedef_doogat(&id, schema);
        let content = parser::serialize(&schema_doogat);
        self.repo.commit_file(
            &typedef_path,
            &content,
            &format!("alter table {table_name}"),
        )?;
        let parsed = parser::parse(&content, &typedef_path)?;
        self.index.index_doogat(&parsed)?;
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
            .ok_or_else(|| DoogatError::SqlEngine(format!("column not found: {column_name}")))?;
        let zone = match zone_str.to_lowercase().as_str() {
            "frontmatter" => Zone::Frontmatter,
            "body" => Zone::Body,
            "reference" => Zone::Reference,
            _ => {
                return Err(DoogatError::SqlEngine(format!(
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
        let mut files: Vec<(String, String)> = Vec::with_capacity(data_doogats.len() + 1);
        files.push((typedef_path.to_string(), typedef_content.clone()));

        for (_, path) in &data_doogats {
            let content = self.repo.read_file(path)?;
            let mut parsed = parser::parse(&content, path)?;
            rename_key_in_doogat(&mut parsed, old_name, new_name, &zone);
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
        self.index.index_doogat(&parsed_typedef)?;
        for (_, path) in &data_doogats {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_doogat(&parsed)?;
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
            return Err(DoogatError::SqlEngine(format!(
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

        // Find all data doogats of this type
        let data_doogats: Vec<(String, String)> = {
            let mut stmt = self
                .index
                .conn
                .prepare("SELECT id, path FROM doogats WHERE type = ?1")?;
            let rows: Vec<(String, String)> = stmt
                .query_map(params![table_name], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };

        if cascade {
            // Delete typedef + all data doogats
            let mut paths: Vec<&str> = vec![&typedef_path];
            for (_, path) in &data_doogats {
                paths.push(path);
            }
            self.repo
                .delete_files(&paths, &format!("drop table {table_name} cascade"))?;

            // Remove from index
            self.index.remove_doogat(&typedef_id)?;
            for (id, _) in &data_doogats {
                self.index.remove_doogat(id)?;
            }
        } else {
            // Rewrite data doogats to remove type field, then delete typedef
            let mut writes: Vec<(String, String)> = Vec::new();
            for (_, path) in &data_doogats {
                let content = self.repo.read_file(path)?;
                let mut parsed = parser::parse(&content, path)?;
                parsed.meta.doogat_type = None;
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

            // Re-index modified data doogats
            for (_, path) in &data_doogats {
                let content = self.repo.read_file(path)?;
                let parsed = parser::parse(&content, path)?;
                self.index.index_doogat(&parsed)?;
            }
            // Remove typedef from index
            self.index.remove_doogat(&typedef_id)?;
        }

        // Drop junction tables for REFERENCES columns
        if let Some(ref schema) = schema {
            for col in &schema.columns {
                if col.references.is_some() {
                    self.index.conn.execute(
                        &format!(
                            "DROP TABLE IF EXISTS \"{table_name}_{col_name}\"",
                            col_name = col.name
                        ),
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
                "SELECT id, path FROM doogats WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| DoogatError::SqlEngine(format!("table not found: {table_name}")))
    }

    /// Resolve doogat ids and paths matching a WHERE clause via SQLite.
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

    fn load_schema(&mut self, table_name: &str) -> Result<TableSchema> {
        let (_id, path) = self.load_typedef_location(table_name)?;
        let content = self.repo.read_file(&path)?;
        let parsed = parser::parse(&content, &path)?;
        schema_from_parsed(&parsed)
    }

    /// Coerce BOOLEAN columns in SELECT results from "1"/"0" to "true"/"false".
    /// Also returns column type metadata when a schema is available.
    /// Extracts table name from the statement's FROM clause, loads its schema,
    /// and applies coercion to matching columns. Falls back to uncoerced rows
    /// when the table can't be determined or has no typedef.
    /// Note: aliased columns (SELECT active AS is_done) won't match the schema
    /// and will skip coercion.
    fn coerce_boolean_columns(
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

    fn cascade_junction_cleanup(&mut self, target_type: &str, deleted_id: &str) -> Result<()> {
        self.index
            .cascade_junction_cleanup(self.repo, target_type, deleted_id)
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
                continue;
            }
            parsed.reference_section = new_section;
            let new_content = parser::serialize(&parsed);

            // Re-index
            let re_parsed = parser::parse(&new_content, source_path)?;
            self.index.index_doogat(&re_parsed)?;

            // Rematerialize if typed
            if let Some(ref stype) = re_parsed.meta.doogat_type {
                if let Ok(schema) = self.load_schema(stype) {
                    self.index
                        .materialize_single(&schema, source_id, &re_parsed)?;
                }
            }

            edits.push(PendingWrite {
                path: source_path.clone(),
                content: new_content,
            });
        }
        Ok(edits)
    }

    /// Check if `table_name` is a junction table (`{type}_{col}` where type is
    /// a known typedef and col is a REFERENCES column in that typedef).
    /// Returns `Some((type_name, col_name))` if it is.
    /// Collect referenced types that use folder storage (for path-qualified wikilinks).
    fn ref_folder_types(&self, schema: &TableSchema) -> std::collections::HashSet<String> {
        schema
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref())
            .filter(|ref_table| self.index.type_uses_folder(ref_table, self.repo))
            .cloned()
            .collect()
    }

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
                    return Ok(Some((
                        candidate_type.to_string(),
                        candidate_col.to_string(),
                    )));
                }
            }
        }
        Ok(None)
    }

    /// Handle INSERT into a junction table by appending reference lines to the
    /// parent doogat and re-indexing.
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
                        rows.push(eval_values(&self.index.conn, row)?);
                    }
                    rows
                }
                _ => {
                    return Err(DoogatError::SqlEngine(
                        "only VALUES clause supported for junction INSERT".into(),
                    ))
                }
            },
            None => return Err(DoogatError::SqlEngine("missing VALUES clause".into())),
        };

        let schema = self.load_schema(type_name)?;
        let ref_col = schema
            .columns
            .iter()
            .find(|c| c.name == col_name && c.references.is_some())
            .ok_or_else(|| {
                DoogatError::SqlEngine(format!(
                    "column {col_name} not found or not a REFERENCES column"
                ))
            })?;
        let ref_folder_types = self.ref_folder_types(&schema);

        let mut affected = 0;
        for row_values in &rows {
            let parent_id_idx = col_names
                .iter()
                .position(|c| *c == type_id_col)
                .ok_or_else(|| {
                    DoogatError::SqlEngine(format!("missing {type_id_col} column in INSERT"))
                })?;
            let target_id_idx =
                col_names
                    .iter()
                    .position(|c| *c == ref_id_col)
                    .ok_or_else(|| {
                        DoogatError::SqlEngine(format!("missing {ref_id_col} column in INSERT"))
                    })?;

            let parent_id = &row_values[parent_id_idx];
            let target_id = &row_values[target_id_idx];

            // Read parent doogat (txn-aware: picks up buffered writes)
            let path = self.index.resolve_path(parent_id)?;
            let content = self.read_content(&path)?;
            let mut parsed = parser::parse(&content, &path)?;

            // Build the reference line with folder-qualified link if needed
            let link_target = if let Some(ref ref_table) = ref_col.references {
                if ref_folder_types.contains(ref_table) {
                    format!("ddb/{ref_table}/{target_id}.md")
                } else {
                    target_id.clone()
                }
            } else {
                target_id.clone()
            };
            let ref_line = format!("- {}:: [[{}]]", col_name, link_target);

            // Skip if reference line already exists (idempotent, 0 affected)
            if parsed
                .reference_section
                .lines()
                .any(|line| line.trim() == ref_line.trim())
            {
                continue;
            }

            // Append to reference section
            let trimmed = parsed.reference_section.trim_end();
            parsed.reference_section = if trimmed.is_empty() {
                format!("{ref_line}\n")
            } else {
                format!("{trimmed}\n{ref_line}\n")
            };

            // Serialize
            let new_content = parser::serialize(&parsed);

            // Re-index this doogat
            let re_parsed = parser::parse(&new_content, &path)?;
            self.index.index_doogat(&re_parsed)?;
            self.index
                .materialize_single(&schema, parent_id, &re_parsed)?;

            if let Some(ref mut buf) = self.txn {
                buf.writes.push(PendingWrite {
                    path,
                    content: new_content,
                });
            } else {
                self.repo.commit_file(
                    &path,
                    &new_content,
                    &format!("add {col_name} ref {target_id} to {type_name} {parent_id}"),
                )?;
            }

            affected += 1;
        }

        Ok(SqlResult::Affected(affected))
    }

    /// Handle DELETE from a junction table by removing matching reference lines
    /// from the parent doogat and re-indexing.
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
                DoogatError::SqlEngine(format!(
                    "column {col_name} not found or not a REFERENCES column"
                ))
            })?;
        let ref_folder_types = self.ref_folder_types(&schema);

        // Read parent doogat (txn-aware: picks up buffered writes)
        let path = self.index.resolve_path(parent_id)?;
        let content = self.read_content(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        // Build the reference line pattern to remove
        let link_target = if let Some(ref ref_table) = ref_col.references {
            if ref_folder_types.contains(ref_table) {
                format!("ddb/{ref_table}/{target_id}.md")
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
        let new_section = if new_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", new_lines.join("\n"))
        };

        // Skip commit if nothing changed
        if new_section == old_section {
            return Ok(SqlResult::Affected(0));
        }
        parsed.reference_section = new_section;

        // Serialize
        let new_content = parser::serialize(&parsed);

        // Re-index
        let re_parsed = parser::parse(&new_content, &path)?;
        self.index.index_doogat(&re_parsed)?;
        self.index
            .materialize_single(&schema, parent_id, &re_parsed)?;

        if let Some(ref mut buf) = self.txn {
            buf.writes.push(PendingWrite {
                path,
                content: new_content,
            });
        } else {
            self.repo.commit_file(
                &path,
                &new_content,
                &format!("remove {col_name} ref {target_id} from {type_name} {parent_id}"),
            )?;
        }

        Ok(SqlResult::Affected(1))
    }

    fn insert_materialized_row(
        &mut self,
        schema: &TableSchema,
        id: &str,
        col_values: &BTreeMap<String, String>,
    ) -> Result<()> {
        // Fetch core fields from the doogats table (populated by index_doogat)
        let (title, date, updated_at): (Option<String>, Option<String>, Option<String>) = self
            .index
            .conn
            .query_row(
                "SELECT title, date, updated_at FROM doogats WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((None, None, None));

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
        let mut vals: Vec<Option<String>> = vec![Some(id.to_string()), title, date, updated_at];

        let mut param_idx = 5;
        for col in &schema.columns {
            if is_core_column(&col.name) {
                continue;
            }
            col_names.push(format!("\"{}\"", col.name));
            placeholders.push(format!("?{}", param_idx));
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
        let valid_cols: Vec<&String> = schema
            .columns
            .iter()
            .filter(|c| !is_core_column(&c.name))
            .map(|c| &c.name)
            .collect();
        let mut set_clauses = Vec::new();
        let mut vals: Vec<String> = Vec::new();

        // Refresh core columns from the doogats table
        if let Ok((title, date, updated_at)) = self.index.conn.query_row(
            "SELECT title, date, updated_at FROM doogats WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        ) {
            if let Some(t) = title {
                vals.push(t);
                set_clauses.push(format!("title = ?{}", vals.len()));
            }
            if let Some(d) = date {
                vals.push(d);
                set_clauses.push(format!("date = ?{}", vals.len()));
            }
            if let Some(u) = updated_at {
                vals.push(u);
                set_clauses.push(format!("updated_at = ?{}", vals.len()));
            }
        }

        for (col, val) in updates {
            if valid_cols.contains(&col) {
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

fn extract_default(options: &[sqlparser::ast::ColumnOptionDef]) -> Result<Option<String>> {
    for opt in options {
        if let ColumnOption::Default(expr) = &opt.option {
            // Bare DEFAULT NEXT
            if let Expr::Identifier(ident) = expr {
                if ident.value.eq_ignore_ascii_case("next") {
                    return Ok(Some("NEXT".to_string()));
                }
            }
            // DEFAULT NEXT(partition_col)
            if let Expr::Function(func) = expr {
                let func_name = func.name.to_string();
                if func_name.eq_ignore_ascii_case("next") {
                    if let sqlparser::ast::FunctionArguments::List(arg_list) = &func.args {
                        if arg_list.args.is_empty() {
                            return Err(DoogatError::SqlEngine(
                                "DEFAULT NEXT() requires exactly one partition column argument"
                                    .into(),
                            ));
                        }
                        if arg_list.args.len() > 1 {
                            return Err(DoogatError::SqlEngine(
                                "DEFAULT NEXT() accepts only one partition column argument".into(),
                            ));
                        }
                        if let Some(sqlparser::ast::FunctionArg::Unnamed(
                            sqlparser::ast::FunctionArgExpr::Expr(Expr::Identifier(ident)),
                        )) = arg_list.args.first()
                        {
                            return Ok(Some(format!("NEXT({})", ident.value)));
                        }
                        return Err(DoogatError::SqlEngine(
                            "DEFAULT NEXT() argument must be a column name".into(),
                        ));
                    }
                }
            }
            return Ok(expr_to_string(expr).ok());
        }
    }
    Ok(None)
}

/// Allowlisted scalar functions that may appear in INSERT/UPDATE expressions.
const ALLOWED_SCALAR_FUNCTIONS: &[&str] = &[
    "COALESCE", "IFNULL", "NULLIF", "ABS", "LENGTH", "LOWER", "UPPER", "TRIM", "TYPEOF", "MIN",
    "MAX",
];

/// Returns true for expressions that are simple literals (no SQLite evaluation needed).
fn is_literal_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Value(_) => true,
        Expr::UnaryOp { expr, .. } => is_literal_expr(expr),
        _ => false,
    }
}

/// Format any expression as valid SQL text (with proper quoting for literals).
fn value_to_sql(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::SingleQuotedString(s) => {
                let escaped = s.replace('\'', "''");
                Ok(format!("'{escaped}'"))
            }
            SqlValue::DoubleQuotedString(s) => {
                let escaped = s.replace('\'', "''");
                Ok(format!("'{escaped}'"))
            }
            SqlValue::Number(n, _) => Ok(n.clone()),
            SqlValue::Boolean(b) => Ok(if *b { "1" } else { "0" }.to_string()),
            SqlValue::Null => Ok("NULL".to_string()),
            _ => Err(DoogatError::SqlEngine(format!("unsupported value: {v}"))),
        },
        Expr::UnaryOp { op, expr } => {
            let inner = value_to_sql(expr)?;
            Ok(format!("{op}{inner}"))
        }
        Expr::Function(func) => {
            let func_name = func.name.to_string().to_uppercase();
            if !ALLOWED_SCALAR_FUNCTIONS.contains(&func_name.as_str()) {
                return Err(DoogatError::SqlEngine(format!(
                    "function not allowed: {func_name}. Allowed: {}",
                    ALLOWED_SCALAR_FUNCTIONS.join(", ")
                )));
            }
            let args = match &func.args {
                sqlparser::ast::FunctionArguments::List(arg_list) => {
                    let mut parts = Vec::new();
                    for arg in &arg_list.args {
                        match arg {
                            sqlparser::ast::FunctionArg::Unnamed(
                                sqlparser::ast::FunctionArgExpr::Expr(e),
                            ) => parts.push(value_to_sql(e)?),
                            _ => {
                                return Err(DoogatError::SqlEngine(format!(
                                    "unsupported function argument in {func_name}"
                                )))
                            }
                        }
                    }
                    parts.join(", ")
                }
                sqlparser::ast::FunctionArguments::None => String::new(),
                _ => {
                    return Err(DoogatError::SqlEngine(format!(
                        "unsupported function argument style in {func_name}"
                    )))
                }
            };
            Ok(format!("{func_name}({args})"))
        }
        Expr::Subquery(query) => Ok(format!("({query})")),
        Expr::BinaryOp { left, op, right } => {
            let l = value_to_sql(left)?;
            let r = value_to_sql(right)?;
            Ok(format!("({l} {op} {r})"))
        }
        Expr::Nested(inner) => {
            let s = value_to_sql(inner)?;
            Ok(format!("({s})"))
        }
        Expr::Identifier(ident) => Ok(format!("\"{}\"", ident.value)),
        _ => Err(DoogatError::SqlEngine(format!(
            "unsupported expression: {expr}"
        ))),
    }
}

fn expr_to_string(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::SingleQuotedString(s) => Ok(s.clone()),
            SqlValue::DoubleQuotedString(s) => Ok(s.clone()),
            SqlValue::Number(n, _) => Ok(n.clone()),
            SqlValue::Boolean(b) => Ok(b.to_string()),
            SqlValue::Null => Ok(String::new()),
            _ => Err(DoogatError::SqlEngine(format!("unsupported value: {v}"))),
        },
        Expr::UnaryOp { op, expr } => {
            let inner = expr_to_string(expr)?;
            Ok(format!("{op}{inner}"))
        }
        Expr::Function(_) | Expr::Subquery(_) | Expr::BinaryOp { .. } | Expr::Nested(_) => {
            value_to_sql(expr)
        }
        _ => Err(DoogatError::SqlEngine(format!(
            "unsupported expression: {expr}"
        ))),
    }
}

/// Convert a rusqlite Value to a String for use as a frontmatter field value.
fn sqlite_value_to_string(result: rusqlite::types::Value) -> Result<String> {
    match result {
        rusqlite::types::Value::Text(s) => Ok(s),
        rusqlite::types::Value::Integer(n) => Ok(n.to_string()),
        rusqlite::types::Value::Real(f) => Ok(f.to_string()),
        rusqlite::types::Value::Null => Ok(String::new()),
        rusqlite::types::Value::Blob(_) => Err(DoogatError::SqlEngine(
            "BLOB result not supported in expression".into(),
        )),
    }
}

/// Evaluate a SQL expression, using SQLite for complex expressions.
/// Simple literals are returned directly without a SQLite roundtrip.
fn eval_expr(conn: &rusqlite::Connection, expr: &Expr) -> Result<String> {
    if is_literal_expr(expr) {
        return expr_to_string(expr);
    }
    let sql = value_to_sql(expr)?;
    let result: rusqlite::types::Value = conn
        .query_row(&format!("SELECT {sql}"), [], |row| row.get(0))
        .map_err(|e| DoogatError::SqlEngine(format!("expression eval failed: {e}")))?;
    sqlite_value_to_string(result)
}

fn eval_values(conn: &rusqlite::Connection, exprs: &[Expr]) -> Result<Vec<String>> {
    exprs.iter().map(|e| eval_expr(conn, e)).collect()
}

fn extract_where_id(selection: &Option<Expr>) -> Result<String> {
    match selection {
        Some(Expr::BinaryOp { left, op, right }) => {
            if format!("{op}") != "=" {
                return Err(DoogatError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            let col = match left.as_ref() {
                Expr::Identifier(ident) => ident.value.to_lowercase(),
                _ => {
                    return Err(DoogatError::SqlEngine(
                        "WHERE clause must be id = '<value>'".into(),
                    ))
                }
            };
            if col != "id" {
                return Err(DoogatError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            expr_to_string(right)
        }
        _ => Err(DoogatError::SqlEngine(
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
                _ => Err(DoogatError::SqlEngine(format!(
                    "junction DELETE requires WHERE {col1_name} = '...' AND {col2_name} = '...'"
                ))),
            }
        }
        _ => Err(DoogatError::SqlEngine(format!(
            "junction DELETE requires WHERE {col1_name} = '...' AND {col2_name} = '...'"
        ))),
    }
}

/// Build a _typedef doogat from a TableSchema.
pub fn build_typedef_doogat(id: &DoogatId, schema: &TableSchema) -> ParsedDoogat {
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

    if let Some(ref constraints) = schema.unique_together {
        if !constraints.is_empty() {
            let outer = Value::List(
                constraints
                    .iter()
                    .map(|cols| {
                        Value::List(cols.iter().map(|c| Value::String(c.clone())).collect())
                    })
                    .collect(),
            );
            extra.insert("unique_together".to_string(), outer);
        }
    }

    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(id.clone()),
            title: Some(schema.table_name.clone()),
            date: None,
            doogat_type: Some("_typedef".into()),
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
        path: format!("ddb/_typedef/{}.md", id.0),
        updated_at: None,
    }
}

/// Build a data doogat from column values according to the schema's zone mapping.
fn build_data_doogat(
    id: &DoogatId,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    ref_folder_types: &std::collections::HashSet<String>,
) -> ParsedDoogat {
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

        match col.effective_zone() {
            Zone::Reference => {
                let link_target = if let Some(ref ref_table) = col.references {
                    if ref_folder_types.contains(ref_table) {
                        format!("ddb/{ref_table}/{val}.md")
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

    // Derive date: schema column "date" in extra > ad-hoc INSERT "date" column > ID-derived
    let date = extra
        .remove("date")
        .and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
        .or_else(|| col_values.get("date").cloned())
        .or_else(|| Some(format!("{}-{}-{}", &id.0[0..4], &id.0[4..6], &id.0[6..8])));

    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(id.clone()),
            title: title_value,
            date,
            doogat_type: Some(schema.table_name.clone()),
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
        path: format!("ddb/{}.md", id.0),
        updated_at: None,
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

use crate::indexer::materialize::{is_core_column, normalize_bool_str};

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

/// Extract a TableSchema from a parsed _typedef doogat.
pub fn schema_from_parsed(doogat: &ParsedDoogat) -> Result<TableSchema> {
    let table_name = doogat
        .meta
        .title
        .as_deref()
        .ok_or_else(|| DoogatError::SqlEngine("typedef doogat missing title".into()))?
        .to_string();

    let columns_val = doogat
        .meta
        .extra
        .get("columns")
        .ok_or_else(|| DoogatError::SqlEngine("typedef doogat missing columns".into()))?;

    let columns_seq = columns_val
        .as_sequence()
        .ok_or_else(|| DoogatError::SqlEngine("columns must be a sequence".into()))?;

    let mut columns = Vec::new();
    for item in columns_seq {
        let map = item
            .as_mapping()
            .ok_or_else(|| DoogatError::SqlEngine("column must be a mapping".into()))?;
        let name = map
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DoogatError::SqlEngine("column missing name".into()))?
            .to_string();
        let data_type = map
            .get("data_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DoogatError::SqlEngine("column missing data_type".into()))?
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

    let crdt_strategy = doogat
        .meta
        .extra
        .get("crdt_strategy")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let template_sections = doogat
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

    let folder = doogat
        .meta
        .extra
        .get("folder")
        .map(|v| matches!(v, crate::types::Value::Bool(true)) || v.as_str() == Some("true"))
        .unwrap_or(false);

    let stale_after_days = doogat
        .meta
        .extra
        .get("stale_after_days")
        .and_then(|v| v.as_f64())
        .map(|n| n as u32);

    let title_template = doogat
        .meta
        .extra
        .get("title_template")
        .and_then(|v| v.as_str())
        .map(String::from);

    let origin = doogat
        .meta
        .extra
        .get("origin")
        .and_then(|v| v.as_str())
        .map(String::from);

    let unique_together = doogat
        .meta
        .extra
        .get("unique_together")
        .and_then(|v| v.as_sequence())
        .and_then(|outer| {
            if outer.is_empty() {
                return None;
            }
            let is_flat = outer.iter().all(|item| item.as_str().is_some());
            let constraints = if is_flat {
                let cols: Vec<String> = outer
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                if cols.is_empty() {
                    return None;
                }
                vec![cols]
            } else {
                outer
                    .iter()
                    .filter_map(|item| item.as_sequence())
                    .map(|inner| {
                        inner
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                    })
                    .filter(|cols| !cols.is_empty())
                    .collect::<Vec<_>>()
            };
            if constraints.is_empty() {
                None
            } else {
                Some(constraints)
            }
        });

    Ok(TableSchema {
        table_name,
        columns,
        crdt_strategy,
        template_sections,
        folder,
        stale_after_days,
        title_template,
        origin,
        unique_together,
    })
}

/// Apply UPDATE SET assignments to a ParsedDoogat according to schema zone mapping.
fn apply_updates_to_doogat(
    doogat: &mut ParsedDoogat,
    schema: &TableSchema,
    updates: &BTreeMap<String, String>,
) {
    for (col_name, new_val) in updates {
        // Handle implicit `title` column (not in schema.columns).
        if col_name == "title" {
            doogat.meta.title = Some(new_val.clone());
            continue;
        }

        let col_def = schema.columns.iter().find(|c| c.name == *col_name);
        let col_def = match col_def {
            Some(c) => c,
            None => continue,
        };

        match col_def.effective_zone() {
            Zone::Reference => {
                update_reference_line(&mut doogat.reference_section, col_name, new_val);
            }
            Zone::Frontmatter => {
                doogat
                    .meta
                    .extra
                    .insert(col_name.clone(), to_yaml_value(new_val, &col_def.data_type));
            }
            Zone::Body => {
                update_body_section(&mut doogat.body, col_name, new_val);
                if let Some(first_body) = schema
                    .columns
                    .iter()
                    .find(|c| c.effective_zone() == Zone::Body)
                {
                    if first_body.name == *col_name {
                        doogat.meta.title = Some(new_val.clone());
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

/// Rename a key in a parsed doogat within the appropriate zone.
fn rename_key_in_doogat(doogat: &mut ParsedDoogat, old_name: &str, new_name: &str, zone: &Zone) {
    match zone {
        Zone::Frontmatter => {
            if let Some(val) = doogat.meta.extra.remove(old_name) {
                doogat.meta.extra.insert(new_name.to_string(), val);
            }
        }
        Zone::Body => {
            let old_heading = format!("## {old_name}");
            let new_heading = format!("## {new_name}");
            doogat.body = doogat.body.replace(&old_heading, &new_heading);
        }
        Zone::Reference => {
            let old_prefix = format!("- {old_name}::");
            let new_prefix = format!("- {new_name}::");
            doogat.reference_section = doogat.reference_section.replace(&old_prefix, &new_prefix);
        }
    }
}

#[cfg(test)]
mod tests;
