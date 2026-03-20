use std::path::Path;
use std::sync::Mutex;

use crate::error::ZettelError;
use crate::service::ZettelService;
use crate::sql_engine::SqlResult;

/// FFI error enum exposed to Swift/Kotlin via UniFFI.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ZdbError {
    #[error("Git: {msg}")]
    Git { msg: String },
    #[error("Yaml: {msg}")]
    Yaml { msg: String },
    #[error("Sql: {msg}")]
    Sql { msg: String },
    #[error("Automerge: {msg}")]
    Automerge { msg: String },
    #[error("Io: {msg}")]
    Io { msg: String },
    #[error("Parse: {msg}")]
    Parse { msg: String },
    #[error("NotFound: {msg}")]
    NotFound { msg: String },
    #[error("Config: {msg}")]
    Config { msg: String },
    #[error("Validation: {msg}")]
    Validation { msg: String },
    #[error("SqlEngine: {msg}")]
    SqlEngine { msg: String },
    #[error("VersionMismatch: {msg}")]
    VersionMismatch { msg: String },
}

impl From<ZettelError> for ZdbError {
    fn from(e: ZettelError) -> Self {
        match e {
            ZettelError::Git(msg) => ZdbError::Git { msg },
            ZettelError::Yaml(msg) => ZdbError::Yaml { msg },
            ZettelError::Sql(msg) => ZdbError::Sql { msg },
            ZettelError::Automerge(msg) => ZdbError::Automerge { msg },
            ZettelError::Io(e) => ZdbError::Io { msg: e.to_string() },
            ZettelError::Toml(msg) => ZdbError::Config { msg },
            ZettelError::Parse(msg) => ZdbError::Parse { msg },
            ZettelError::NotFound(msg) => ZdbError::NotFound { msg },
            ZettelError::Validation(msg) => ZdbError::Validation { msg },
            ZettelError::InvalidPath(msg) => ZdbError::Validation { msg },
            ZettelError::SqlEngine(msg) => ZdbError::SqlEngine { msg },
            ZettelError::VersionMismatch { repo, driver } => ZdbError::VersionMismatch {
                msg: format!("repo format v{repo}, driver supports up to v{driver}"),
            },
            #[cfg(feature = "nosql")]
            ZettelError::Redb(msg) => ZdbError::Io { msg },
        }
    }
}

/// FFI-safe search result.
#[derive(uniffi::Record)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub rank: f64,
}

impl From<crate::types::SearchResult> for SearchResult {
    fn from(r: crate::types::SearchResult) -> Self {
        Self {
            id: r.id,
            title: r.title,
            path: r.path,
            snippet: r.snippet,
            rank: r.rank,
        }
    }
}

/// FFI-safe paginated search result.
#[derive(uniffi::Record)]
pub struct PaginatedSearchResult {
    pub hits: Vec<SearchResult>,
    pub total_count: u64,
}

impl From<crate::types::PaginatedSearchResult> for PaginatedSearchResult {
    fn from(r: crate::types::PaginatedSearchResult) -> Self {
        Self {
            hits: r.hits.into_iter().map(Into::into).collect(),
            total_count: r.total_count as u64,
        }
    }
}

/// FFI-safe rebuild report.
#[derive(uniffi::Record)]
pub struct RebuildReport {
    pub indexed: u64,
    pub tables_materialized: u64,
    pub types_inferred: Vec<String>,
}

/// FFI-safe attachment metadata.
#[derive(uniffi::Record)]
pub struct AttachmentInfo {
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub path: String,
}

impl From<crate::types::AttachmentInfo> for AttachmentInfo {
    fn from(a: crate::types::AttachmentInfo) -> Self {
        Self {
            name: a.name,
            mime: a.mime,
            size: a.size,
            path: a.path,
        }
    }
}

/// FFI-safe column definition for type schema discovery.
#[derive(uniffi::Record)]
pub struct ColumnDefRecord {
    pub name: String,
    pub data_type: String,
    pub references: Option<String>,
    pub required: bool,
}

impl From<&crate::types::ColumnDef> for ColumnDefRecord {
    fn from(c: &crate::types::ColumnDef) -> Self {
        Self {
            name: c.name.clone(),
            data_type: c.data_type.clone(),
            references: c.references.clone(),
            required: c.required,
        }
    }
}

/// FFI-safe type schema for typedef discovery.
#[derive(uniffi::Record)]
pub struct TypeSchemaRecord {
    pub table_name: String,
    pub columns: Vec<ColumnDefRecord>,
    pub crdt_strategy: Option<String>,
    pub template_sections: Vec<String>,
}

impl From<crate::types::TableSchema> for TypeSchemaRecord {
    fn from(s: crate::types::TableSchema) -> Self {
        Self {
            table_name: s.table_name,
            columns: s.columns.iter().map(ColumnDefRecord::from).collect(),
            crdt_strategy: s.crdt_strategy,
            template_sections: s.template_sections,
        }
    }
}

/// FFI-safe SQL execution result.
///
/// Flat record suitable for UniFFI export. Queries populate `columns`/`rows`;
/// mutations populate `affected_rows`; DDL sets `message`.
#[derive(uniffi::Record)]
pub struct SqlResultRecord {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub affected_rows: i64,
    pub message: String,
}

impl From<SqlResult> for SqlResultRecord {
    fn from(r: SqlResult) -> Self {
        match r {
            SqlResult::Rows { columns, rows } => {
                let affected_rows = rows.len() as i64;
                Self {
                    columns,
                    rows,
                    affected_rows,
                    message: String::new(),
                }
            }
            SqlResult::Affected(n) => Self {
                columns: Vec::new(),
                rows: Vec::new(),
                affected_rows: n as i64,
                message: String::new(),
            },
            SqlResult::Ok(msg) => Self {
                columns: Vec::new(),
                rows: Vec::new(),
                affected_rows: 0,
                message: msg,
            },
        }
    }
}

/// High-level facade for mobile/desktop FFI consumers.
///
/// Wraps a single `Mutex<ZettelService>` for thread safety.
#[derive(uniffi::Object)]
pub struct ZettelDriver {
    svc: Mutex<ZettelService>,
}

#[uniffi::export]
impl ZettelDriver {
    /// Open an existing ZettelDB repository.
    #[uniffi::constructor]
    pub fn new(repo_path: String) -> Result<Self, ZdbError> {
        let svc = ZettelService::open(Path::new(&repo_path)).map_err(ZdbError::from)?;
        Ok(Self {
            svc: Mutex::new(svc),
        })
    }

    /// Initialize a new ZettelDB repository at `repo_path` and open it.
    #[uniffi::constructor]
    pub fn create_repo(repo_path: String) -> Result<Self, ZdbError> {
        let svc = ZettelService::init(Path::new(&repo_path)).map_err(ZdbError::from)?;
        Ok(Self {
            svc: Mutex::new(svc),
        })
    }

    pub fn create_zettel(&self, content: String, message: String) -> Result<String, ZdbError> {
        let svc = self.svc.lock().unwrap();
        svc.create_zettel_raw(&content, &message)
            .map_err(ZdbError::from)
    }

    pub fn read_zettel(&self, id: String) -> Result<String, ZdbError> {
        let svc = self.svc.lock().unwrap();
        svc.read_zettel(&id).map_err(ZdbError::from)
    }

    pub fn update_zettel(
        &self,
        id: String,
        content: String,
        message: String,
    ) -> Result<(), ZdbError> {
        let svc = self.svc.lock().unwrap();
        svc.update_zettel_raw(&id, &content, &message)
            .map_err(ZdbError::from)
    }

    pub fn delete_zettel(&self, id: String, message: String) -> Result<(), ZdbError> {
        let svc = self.svc.lock().unwrap();
        let rel_path = svc.resolve_path(&id).map_err(ZdbError::from)?;
        svc.repo()
            .delete_file(&rel_path, &message)
            .map_err(ZdbError::from)?;
        svc.index().remove_zettel(&id).map_err(ZdbError::from)
    }

    pub fn search(&self, query: String) -> Result<Vec<SearchResult>, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let results = svc.search(&query).map_err(ZdbError::from)?;
        Ok(results.into_iter().map(Into::into).collect())
    }

    pub fn search_paginated(
        &self,
        query: String,
        limit: u32,
        offset: u32,
    ) -> Result<PaginatedSearchResult, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let result = svc
            .search_paginated(&query, limit as usize, offset as usize)
            .map_err(ZdbError::from)?;
        Ok(result.into())
    }

    pub fn reindex(&self) -> Result<RebuildReport, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let report = svc.reindex().map_err(ZdbError::from)?;
        Ok(RebuildReport {
            indexed: report.indexed as u64,
            tables_materialized: report.tables_materialized as u64,
            types_inferred: report.types_inferred,
        })
    }

    pub fn register_node(&self, name: String) -> Result<String, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let node = svc.register_node(&name).map_err(ZdbError::from)?;
        Ok(node.uuid)
    }

    pub fn compact(&self) -> Result<(), ZdbError> {
        let svc = self.svc.lock().unwrap();
        let opts = crate::types::CompactOptions {
            force: true,
            skip_backup: true,
            ..Default::default()
        };
        svc.compact(&opts).map_err(ZdbError::from)?;
        Ok(())
    }

    pub fn run_maintenance(&self) -> Result<bool, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let report = svc.run_maintenance(None).map_err(ZdbError::from)?;
        Ok(report.success)
    }

    pub fn list_zettels(&self) -> Result<Vec<String>, ZdbError> {
        let svc = self.svc.lock().unwrap();
        svc.list_zettels().map_err(ZdbError::from)
    }

    pub fn execute_sql(&self, sql: String) -> Result<SqlResultRecord, ZdbError> {
        let mut svc = self.svc.lock().unwrap();
        let result = svc.execute_sql(&sql).map_err(ZdbError::from)?;
        Ok(result.into())
    }

    pub fn begin_transaction(&self) -> Result<(), ZdbError> {
        let mut svc = self.svc.lock().unwrap();
        svc.begin_transaction().map_err(ZdbError::from)
    }

    pub fn commit_transaction(&self) -> Result<(), ZdbError> {
        let mut svc = self.svc.lock().unwrap();
        svc.commit_transaction().map_err(ZdbError::from)
    }

    pub fn rollback_transaction(&self) -> Result<(), ZdbError> {
        let mut svc = self.svc.lock().unwrap();
        svc.rollback_transaction().map_err(ZdbError::from)
    }

    pub fn list_type_schemas(&self) -> Result<Vec<TypeSchemaRecord>, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let schemas = svc.list_type_schemas().map_err(ZdbError::from)?;
        Ok(schemas.into_iter().map(Into::into).collect())
    }

    pub fn attach_file(
        &self,
        zettel_id: String,
        file_path: String,
    ) -> Result<AttachmentInfo, ZdbError> {
        let bytes =
            std::fs::read(&file_path).map_err(|e| ZdbError::from(ZettelError::Io(e)))?;
        let filename = Path::new(&file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ZdbError::Validation {
                msg: "invalid filename".into(),
            })?
            .to_owned();
        let mime = crate::types::AttachmentInfo::mime_from_filename(&filename).to_owned();
        let svc = self.svc.lock().unwrap();
        let info = svc
            .attach_file(&zettel_id, &filename, &bytes, &mime)
            .map_err(ZdbError::from)?;
        Ok(info.into())
    }

    pub fn detach_file(&self, zettel_id: String, filename: String) -> Result<(), ZdbError> {
        let svc = self.svc.lock().unwrap();
        svc.detach_file(&zettel_id, &filename)
            .map_err(ZdbError::from)
    }

    pub fn list_attachments(&self, zettel_id: String) -> Result<Vec<AttachmentInfo>, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let list = svc
            .list_attachments(&zettel_id)
            .map_err(ZdbError::from)?;
        Ok(list.into_iter().map(AttachmentInfo::from).collect())
    }

    pub fn export_full_bundle(&self, output_path: String) -> Result<String, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let path = svc
            .export_full_bundle(Path::new(&output_path))
            .map_err(ZdbError::from)?;
        Ok(path.to_string_lossy().into_owned())
    }

    pub fn export_delta_bundle(
        &self,
        target_node_uuid: String,
        output_path: String,
    ) -> Result<String, ZdbError> {
        let svc = self.svc.lock().unwrap();
        let path = svc
            .export_delta_bundle(&target_node_uuid, Path::new(&output_path))
            .map_err(ZdbError::from)?;
        Ok(path.to_string_lossy().into_owned())
    }

    pub fn import_bundle(&self, bundle_path: String) -> Result<(), ZdbError> {
        let svc = self.svc.lock().unwrap();
        svc.import_bundle(Path::new(&bundle_path))
            .map_err(ZdbError::from)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_driver() -> (TempDir, ZettelDriver) {
        let tmp = TempDir::new().unwrap();
        let driver = ZettelDriver::create_repo(tmp.path().to_str().unwrap().to_string()).unwrap();
        (tmp, driver)
    }

    #[test]
    fn init_creates_repo_and_opens_driver() {
        let (_tmp, driver) = fresh_driver();
        let list = driver.list_zettels().unwrap();
        assert!(list.is_empty(), "fresh repo should have no zettels");
    }

    #[test]
    fn register_node_returns_uuid() {
        let (_tmp, driver) = fresh_driver();
        let uuid = driver.register_node("TestNode".to_string()).unwrap();
        assert!(!uuid.is_empty(), "uuid should not be empty");
        assert_eq!(uuid.len(), 36, "uuid should be 36 chars");
    }

    // --- SqlEngine-backed execute_sql tests ---

    #[test]
    fn execute_sql_create_table_creates_typedef_zettel() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        let result = driver
            .execute_sql("CREATE TABLE project (name TEXT, status TEXT)".into())
            .unwrap();
        assert!(!result.message.is_empty(), "DDL should return a message");

        // Verify typedef zettel was created on disk (not just in SQLite cache)
        let zettels = driver.list_zettels().unwrap();
        let has_typedef = zettels
            .iter()
            .any(|p| p.contains("_typedef/") && p.contains(".md"));
        assert!(has_typedef, "typedef zettel should exist on disk");
    }

    #[test]
    fn execute_sql_insert_returns_id_and_queryable() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE task (priority TEXT, done TEXT)".into())
            .unwrap();

        let result = driver
            .execute_sql("INSERT INTO task (priority, done) VALUES ('high', 'no')".into())
            .unwrap();
        // INSERT returns SqlResult::Ok(created_ids) — message contains the new zettel ID
        assert!(
            !result.message.is_empty(),
            "INSERT should return created ID"
        );

        // Verify the inserted row is queryable via SqlEngine
        let rows = driver
            .execute_sql("SELECT priority, done FROM task".into())
            .unwrap();
        assert_eq!(rows.rows.len(), 1);
        assert_eq!(rows.rows[0][0], "high");
        assert_eq!(rows.rows[0][1], "no");
    }

    #[test]
    fn execute_sql_select_returns_rows() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE item (name TEXT)".into())
            .unwrap();
        driver
            .execute_sql("INSERT INTO item (name) VALUES ('alpha')".into())
            .unwrap();
        driver
            .execute_sql("INSERT INTO item (name) VALUES ('beta')".into())
            .unwrap();

        let result = driver
            .execute_sql("SELECT name FROM item ORDER BY name".into())
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert!(result.columns.contains(&"name".to_string()));
        assert_eq!(result.rows[0][0], "alpha");
        assert_eq!(result.rows[1][0], "beta");
    }

    #[test]
    fn execute_sql_update_modifies_zettel() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE note (body TEXT)".into())
            .unwrap();
        driver
            .execute_sql("INSERT INTO note (body) VALUES ('original')".into())
            .unwrap();

        let result = driver
            .execute_sql("UPDATE note SET body = 'modified'".into())
            .unwrap();
        assert_eq!(result.affected_rows, 1);

        let select = driver.execute_sql("SELECT body FROM note".into()).unwrap();
        assert_eq!(select.rows[0][0], "modified");
    }

    #[test]
    fn execute_sql_delete_removes_zettel() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE widget (label TEXT)".into())
            .unwrap();
        driver
            .execute_sql("INSERT INTO widget (label) VALUES ('remove-me')".into())
            .unwrap();

        // Get the ID to delete by querying
        let before = driver.execute_sql("SELECT id FROM widget".into()).unwrap();
        assert_eq!(before.rows.len(), 1);
        let id = &before.rows[0][0];

        let result = driver
            .execute_sql(format!("DELETE FROM widget WHERE id = '{id}'"))
            .unwrap();
        assert_eq!(result.affected_rows, 1);

        let after = driver.execute_sql("SELECT id FROM widget".into()).unwrap();
        assert_eq!(after.rows.len(), 0);
    }

    #[test]
    fn execute_sql_invalid_syntax_returns_error() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        let result = driver.execute_sql("NOT VALID SQL AT ALL".into());
        assert!(result.is_err());
    }

    #[test]
    fn execute_sql_dml_on_nonexistent_type_returns_error() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        let result = driver.execute_sql("INSERT INTO nonexistent (x) VALUES ('y')".into());
        assert!(result.is_err());
    }

    // --- Transaction tests ---

    #[test]
    fn transaction_commit_persists_writes() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE txtest (val TEXT)".into())
            .unwrap();

        driver.begin_transaction().unwrap();
        driver
            .execute_sql("INSERT INTO txtest (val) VALUES ('in-txn')".into())
            .unwrap();
        driver.commit_transaction().unwrap();

        let result = driver.execute_sql("SELECT val FROM txtest".into()).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], "in-txn");
    }

    #[test]
    fn transaction_rollback_discards_writes() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE rbtest (val TEXT)".into())
            .unwrap();

        driver.begin_transaction().unwrap();
        driver
            .execute_sql("INSERT INTO rbtest (val) VALUES ('should-vanish')".into())
            .unwrap();
        driver.rollback_transaction().unwrap();

        let result = driver.execute_sql("SELECT val FROM rbtest".into()).unwrap();
        assert_eq!(result.rows.len(), 0, "rolled back insert should not appear");
    }

    #[test]
    fn transaction_multiple_ops_commit_atomically() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE multi (name TEXT)".into())
            .unwrap();

        driver.begin_transaction().unwrap();
        driver
            .execute_sql("INSERT INTO multi (name) VALUES ('one')".into())
            .unwrap();
        driver
            .execute_sql("INSERT INTO multi (name) VALUES ('two')".into())
            .unwrap();
        driver.commit_transaction().unwrap();

        let result = driver
            .execute_sql("SELECT name FROM multi ORDER BY name".into())
            .unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn begin_without_commit_or_rollback_errors_on_double_begin() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver.begin_transaction().unwrap();
        let result = driver.begin_transaction();
        assert!(result.is_err(), "double BEGIN should fail");
        // Clean up: rollback the first transaction
        driver.rollback_transaction().unwrap();
    }

    #[test]
    fn commit_without_begin_errors() {
        let (_tmp, driver) = fresh_driver();
        let result = driver.commit_transaction();
        assert!(result.is_err());
    }

    #[test]
    fn rollback_without_begin_errors() {
        let (_tmp, driver) = fresh_driver();
        let result = driver.rollback_transaction();
        assert!(result.is_err());
    }

    // --- Type discovery tests ---

    #[test]
    fn list_type_schemas_empty_on_fresh_repo() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        let schemas = driver.list_type_schemas().unwrap();
        assert!(schemas.is_empty());
    }

    #[test]
    fn list_type_schemas_returns_created_type() {
        let (_tmp, driver) = fresh_driver();
        driver.reindex().unwrap();
        driver
            .execute_sql("CREATE TABLE contact (name TEXT, email TEXT)".into())
            .unwrap();

        let schemas = driver.list_type_schemas().unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].table_name, "contact");

        let col_names: Vec<_> = schemas[0].columns.iter().map(|c| c.name.as_str()).collect();
        assert!(col_names.contains(&"name"));
        assert!(col_names.contains(&"email"));
    }

    // --- Parity test: FFI path produces same results as direct SqlEngine ---

    #[test]
    fn parity_ffi_and_direct_sqlengine_produce_equivalent_results() {
        // Set up two identical repos: one exercised via ZettelDriver (FFI), one via SqlEngine directly
        let tmp_ffi = TempDir::new().unwrap();
        let driver =
            ZettelDriver::create_repo(tmp_ffi.path().to_str().unwrap().to_string()).unwrap();
        driver.reindex().unwrap();

        let tmp_direct = TempDir::new().unwrap();
        let direct_repo = crate::git_ops::GitRepo::init(tmp_direct.path()).unwrap();
        let db_path = tmp_direct.path().join(".zdb/index.db");
        std::fs::create_dir_all(tmp_direct.path().join(".zdb")).unwrap();
        let direct_index = crate::indexer::Index::open(&db_path).unwrap();
        direct_index.rebuild(&direct_repo).unwrap();

        // Same DDL on both paths
        let ffi_ddl = driver
            .execute_sql("CREATE TABLE workspace (description TEXT)".into())
            .unwrap();
        let mut engine = crate::sql_engine::SqlEngine::new(&direct_index, &direct_repo);
        let direct_ddl = engine
            .execute("CREATE TABLE workspace (description TEXT)")
            .unwrap();
        let direct_ddl: SqlResultRecord = direct_ddl.into();
        assert_eq!(ffi_ddl.message.is_empty(), direct_ddl.message.is_empty());

        // Same INSERT on both paths
        let ffi_ins = driver
            .execute_sql("INSERT INTO workspace (description) VALUES ('shared model')".into())
            .unwrap();
        let mut engine = crate::sql_engine::SqlEngine::new(&direct_index, &direct_repo);
        let direct_ins = engine
            .execute("INSERT INTO workspace (description) VALUES ('shared model')")
            .unwrap();
        let direct_ins: SqlResultRecord = direct_ins.into();
        // Both return created IDs in message
        assert!(!ffi_ins.message.is_empty());
        assert!(!direct_ins.message.is_empty());

        // Same SELECT on both paths — row count and column names must match
        let ffi_sel = driver
            .execute_sql("SELECT description FROM workspace".into())
            .unwrap();
        let mut engine = crate::sql_engine::SqlEngine::new(&direct_index, &direct_repo);
        let direct_sel = engine.execute("SELECT description FROM workspace").unwrap();
        let direct_sel: SqlResultRecord = direct_sel.into();
        assert_eq!(ffi_sel.columns, direct_sel.columns);
        assert_eq!(ffi_sel.rows.len(), direct_sel.rows.len());
        assert_eq!(ffi_sel.rows[0][0], direct_sel.rows[0][0]);

        // Same UPDATE on both paths
        let ffi_upd = driver
            .execute_sql("UPDATE workspace SET description = 'updated'".into())
            .unwrap();
        let mut engine = crate::sql_engine::SqlEngine::new(&direct_index, &direct_repo);
        let direct_upd = engine
            .execute("UPDATE workspace SET description = 'updated'")
            .unwrap();
        let direct_upd: SqlResultRecord = direct_upd.into();
        assert_eq!(ffi_upd.affected_rows, direct_upd.affected_rows);
    }

    // --- Delta bundle export tests ---

    #[test]
    fn export_delta_bundle_targets_node() {
        let (_tmp, driver) = fresh_driver();
        driver.register_node("Local".to_string()).unwrap();

        // Create initial content (before remote's sync point)
        driver
            .create_zettel("---\ntitle: first\n---\nBody1".into(), "add first".into())
            .unwrap();

        // Capture head as remote's sync point
        let sync_point = {
            let svc = driver.svc.lock().unwrap();
            svc.head_oid().unwrap().0.clone()
        };

        // Register a remote node with known_heads at sync_point
        let node2_uuid = "remote-node-2";
        let node2_config = format!(
            "uuid = \"{node2_uuid}\"\nname = \"Node2\"\nknown_heads = [\"{sync_point}\"]\n\
             status = \"Active\"\n"
        );
        {
            let svc = driver.svc.lock().unwrap();
            svc.repo()
                .commit_file(
                    &format!(".nodes/{node2_uuid}.toml"),
                    &node2_config,
                    "register node2",
                )
                .unwrap();
        }

        // Add new content after remote's sync point
        driver
            .create_zettel("---\ntitle: second\n---\nBody2".into(), "add second".into())
            .unwrap();

        // Export delta bundle targeting node2
        let output = _tmp.path().join("delta.bundle.tar");
        let path = driver
            .export_delta_bundle(node2_uuid.to_string(), output.to_str().unwrap().to_string())
            .unwrap();
        assert!(std::path::Path::new(&path).exists());
    }

    #[test]
    fn export_delta_bundle_unknown_node_errors() {
        let (_tmp, driver) = fresh_driver();
        driver.register_node("Local".to_string()).unwrap();
        driver
            .create_zettel("---\ntitle: test\n---\nBody".into(), "add".into())
            .unwrap();

        let output = _tmp.path().join("delta.bundle.tar");
        let result = driver.export_delta_bundle(
            "nonexistent-uuid".to_string(),
            output.to_str().unwrap().to_string(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn export_delta_bundle_smaller_than_full() {
        let (_tmp, driver) = fresh_driver();
        driver.register_node("Local".to_string()).unwrap();

        // Create initial content
        driver
            .create_zettel("---\ntitle: first\n---\nBody1".into(), "add first".into())
            .unwrap();

        // Capture head as remote's sync point
        let sync_point = {
            let svc = driver.svc.lock().unwrap();
            svc.head_oid().unwrap().0.clone()
        };

        // Register remote node with known_heads
        let node2_uuid = "remote-node-2";
        let node2_config = format!(
            "uuid = \"{node2_uuid}\"\nname = \"Node2\"\nknown_heads = [\"{sync_point}\"]\n\
             status = \"Active\"\n"
        );
        {
            let svc = driver.svc.lock().unwrap();
            svc.repo()
                .commit_file(
                    &format!(".nodes/{node2_uuid}.toml"),
                    &node2_config,
                    "register node2",
                )
                .unwrap();
        }

        // Add more content after sync point
        for i in 0..3 {
            driver
                .create_zettel(
                    format!("---\ntitle: note{i}\n---\nContent {i}"),
                    format!("add note{i}"),
                )
                .unwrap();
        }

        // Export both bundle types
        let delta_path = _tmp.path().join("delta.bundle.tar");
        driver
            .export_delta_bundle(
                node2_uuid.to_string(),
                delta_path.to_str().unwrap().to_string(),
            )
            .unwrap();

        let full_path = _tmp.path().join("full.bundle.tar");
        driver
            .export_full_bundle(full_path.to_str().unwrap().to_string())
            .unwrap();

        let delta_size = std::fs::metadata(&delta_path).unwrap().len();
        let full_size = std::fs::metadata(&full_path).unwrap().len();
        assert!(
            delta_size < full_size,
            "delta ({delta_size}B) should be smaller than full ({full_size}B)"
        );
    }
}
