mod builders;
mod ddl;
mod dml;
mod helpers;
mod junction;
mod transaction;
pub(crate) mod typed_insert;

pub(crate) use builders::{apply_updates_to_doogat, build_data_doogat};

use rusqlite::params;
use sqlparser::ast::{ObjectType, Statement};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::traits::DoogatStore;
use crate::traits::SqlBackend;
use crate::types::DoogatId;

pub(crate) use builders::resolve_insert_title;
pub use builders::{build_typedef_doogat, schema_from_parsed};

use helpers::{
    normalize_alter_column_type, re_create_table_singleton, re_drop_search_key, re_drop_singleton,
    re_drop_title_template, re_set_search_key, re_set_singleton, re_set_title_template,
    re_set_zone,
};

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
    pub(super) index: &'a dyn SqlBackend,
    pub(super) repo: &'a dyn DoogatStore,
    pub(super) txn: Option<TransactionBuffer>,
    /// PRD 00139 §2: pre-parse-detected CREATE TABLE SINGLETON marker.
    /// `None` means the next CREATE TABLE is not singleton; `Some(false)`
    /// means `SINGLETON` only; `Some(true)` means `SINGLETON DEFAULT
    /// VALUES`. Set in `execute_batch` after stripping the marker from the
    /// source SQL (so sqlparser sees a valid statement) and consumed by
    /// `handle_create_table`. T7 reads the inner bool to drive the
    /// post-install auto-seed.
    pub(super) pending_singleton: Option<bool>,
}

impl<'a> SqlEngine<'a> {
    pub fn new(index: &'a dyn SqlBackend, repo: &'a dyn DoogatStore) -> Self {
        Self {
            index,
            repo,
            txn: None,
            pending_singleton: None,
        }
    }

    /// Restore a previously extracted transaction buffer.
    /// The caller is responsible for ensuring the SAVEPOINT is still active
    /// on `index.sql_conn()` (i.e. the same connection that created it).
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
                .sql_conn()
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
                    .sql_conn()
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
            let table = caps
                .get(1)
                .or(caps.get(2))
                .expect("regex guarantees group 1 or 2")
                .as_str();
            let zone = caps.get(3).expect("regex guarantees group 3").as_str();
            let column = caps
                .get(4)
                .or(caps.get(5))
                .expect("regex guarantees group 4 or 5")
                .as_str();
            return Ok(Some(self.handle_set_zone(table, zone, column)?));
        }
        if let Some(caps) = re_set_title_template().captures(sql) {
            let table = caps
                .get(1)
                .or(caps.get(2))
                .expect("regex guarantees group 1 or 2")
                .as_str();
            let template = caps.get(3).expect("regex guarantees group 3").as_str();
            return Ok(Some(self.handle_title_template(table, Some(template))?));
        }
        if let Some(caps) = re_drop_title_template().captures(sql) {
            let table = caps
                .get(1)
                .or(caps.get(2))
                .expect("regex guarantees group 1 or 2")
                .as_str();
            return Ok(Some(self.handle_title_template(table, None)?));
        }
        if let Some(caps) = re_set_search_key().captures(sql) {
            let table = caps
                .get(1)
                .or(caps.get(2))
                .expect("regex guarantees group 1 or 2")
                .as_str();
            let column = caps
                .get(3)
                .or(caps.get(4))
                .expect("regex guarantees group 3 or 4")
                .as_str();
            return Ok(Some(self.handle_search_key(table, Some(column))?));
        }
        if let Some(caps) = re_drop_search_key().captures(sql) {
            let table = caps
                .get(1)
                .or(caps.get(2))
                .expect("regex guarantees group 1 or 2")
                .as_str();
            return Ok(Some(self.handle_search_key(table, None)?));
        }
        if let Some(caps) = re_set_singleton().captures(sql) {
            let table = caps
                .get(1)
                .or(caps.get(2))
                .expect("regex guarantees group 1 or 2")
                .as_str();
            return Ok(Some(self.handle_set_singleton(table)?));
        }
        if let Some(caps) = re_drop_singleton().captures(sql) {
            let table = caps
                .get(1)
                .or(caps.get(2))
                .expect("regex guarantees group 1 or 2")
                .as_str();
            return Ok(Some(self.handle_drop_singleton(table)?));
        }
        Ok(None)
    }

    pub fn execute_batch(&mut self, sql: &str) -> Result<Vec<SqlResult>> {
        // Pre-parse interception for custom DDL that sqlparser can't handle
        if let Some(result) = self.try_custom_ddl(sql)? {
            return Ok(vec![result]);
        }

        // PRD 00139 §2: detect and strip the trailing CREATE TABLE
        // SINGLETON [DEFAULT VALUES] marker before sqlparser sees the
        // statement. The flag is parked on `self.pending_singleton` and
        // consumed by `handle_create_table` (T5) and `handle_create_table`
        // auto-seed branch (T7). Single-statement scope: SINGLETON only
        // applies to one CREATE TABLE per call, so per-engine state is fine.
        let stripped = if let Some(caps) = re_create_table_singleton().captures(sql) {
            let default_values = caps.get(1).is_some();
            self.pending_singleton = Some(default_values);
            re_create_table_singleton().replace(sql, ")").into_owned()
        } else {
            self.pending_singleton = None;
            sql.to_string()
        };

        let normalized = normalize_alter_column_type(&stripped);
        let sql_parse = normalized.as_ref();

        let dialect = GenericDialect {};
        let statements = Parser::parse_sql(&dialect, sql_parse)
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
        // PRD 00129 §3b: `CREATE [UNIQUE] INDEX IF NOT EXISTS ...` is
        // accepted as a no-op so apps with legacy startup migrations
        // (jink today) keep booting after upgrade. The intended uniqueness
        // path is `UNIQUE(...)` in the typedef, enforced by the typed
        // create write path. Plain `CREATE INDEX` (no IF NOT EXISTS) is
        // still rejected by `reject_unsupported_ddl` below — that's an
        // intentional declaration the caller should learn to drop.
        if let Statement::CreateIndex(ci) = stmt {
            if ci.if_not_exists {
                let name = ci
                    .name
                    .as_ref()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "<anonymous>".to_string());
                tracing::info!(
                    index_name = %name,
                    "create_index_ignored: managed by typedef UNIQUE/PRIMARY KEY"
                );
                return Ok(SqlResult::Ok(format!(
                    "ignored: index '{name}' is managed by typedef UNIQUE/PRIMARY KEY constraints"
                )));
            }
        }

        if let Some(err) = reject_unsupported_ddl(stmt) {
            return Err(err);
        }
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
            Statement::StartTransaction { .. } => self.handle_begin(),
            Statement::Commit { .. } => self.handle_commit(),
            Statement::Rollback { .. } => self.handle_rollback(),
            _ => {
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
}

fn reject_unsupported_ddl(stmt: &Statement) -> Option<DoogatError> {
    let msg = match stmt {
        Statement::CreateIndex(_) => {
            "CREATE INDEX not supported: indexes on the materialized cache are rebuilt from doogat data on reindex"
        }
        Statement::CreateView { .. } => {
            "CREATE VIEW not supported: views are not stored as doogats and are lost on reindex"
        }
        Statement::CreateVirtualTable { .. } => {
            "CREATE VIRTUAL TABLE not supported: virtual tables have no doogat representation"
        }
        Statement::CreateTrigger { .. } => {
            "CREATE TRIGGER not supported: triggers fire on cache mutations, not git commits"
        }
        Statement::AlterIndex { .. } => {
            "ALTER INDEX not supported: indexes are managed automatically and rebuilt on reindex"
        }
        Statement::RenameTable(_) => {
            "RENAME TABLE not supported: use ALTER TABLE foo RENAME TO bar"
        }
        _ => return None,
    };
    Some(DoogatError::SqlEngine(msg.into()))
}

#[cfg(test)]
mod tests;
