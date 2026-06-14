
use super::*;
use tempfile::TempDir;

fn fresh_driver() -> (TempDir, DoogatDriver) {
    let tmp = TempDir::new().unwrap();
    let driver = DoogatDriver::create_repo(tmp.path().to_str().unwrap().to_string()).unwrap();
    (tmp, driver)
}

fn setup_app_config_singleton_typedef(driver: &DoogatDriver) {
    let typedef = "---
id: 20260510130000
title: app_config
type: _typedef
singleton: true
columns:
  - name: theme
    data_type: TEXT
    zone: frontmatter
---
";
    let typedef_path = "ddb/_typedef/20260510130000.md";
    let svc = driver.svc.lock().unwrap();
    svc.repo
        .commit_file(typedef_path, typedef, "add app_config singleton typedef")
        .unwrap();
    let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
    svc.index.index_doogat(&parsed).unwrap();
    svc.index.materialize_all_types(&svc.repo).unwrap();
}

#[test]
fn init_creates_repo_and_opens_driver() {
    let (_tmp, driver) = fresh_driver();
    let list = driver.list_doogats().unwrap();
    assert!(list.is_empty(), "fresh repo should have no doogats");
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
fn execute_sql_create_table_creates_typedef_doogat() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();
    let result = driver
        .execute_sql("CREATE TABLE project (name TEXT, status TEXT)".into())
        .unwrap();
    assert!(!result.message.is_empty(), "DDL should return a message");

    // Verify typedef doogat was created on disk (not just in SQLite cache)
    let doogats = driver.list_doogats().unwrap();
    let has_typedef = doogats
        .iter()
        .any(|p| p.contains("_typedef/") && p.contains(".md"));
    assert!(has_typedef, "typedef doogat should exist on disk");
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
    // INSERT returns SqlResult::Ok(created_ids) — message contains the new doogat ID
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
fn execute_sql_update_modifies_doogat() {
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
fn execute_sql_delete_removes_doogat() {
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
    // Set up two identical repos: one exercised via DoogatDriver (FFI), one via SqlEngine directly
    let tmp_ffi = TempDir::new().unwrap();
    let driver = DoogatDriver::create_repo(tmp_ffi.path().to_str().unwrap().to_string()).unwrap();
    driver.reindex().unwrap();

    let tmp_direct = TempDir::new().unwrap();
    let direct_repo = crate::git_ops::GitRepo::init(tmp_direct.path()).unwrap();
    let db_path = tmp_direct.path().join(".ddb/index.db");
    std::fs::create_dir_all(tmp_direct.path().join(".ddb")).unwrap();
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
        .create_doogat("---\ntitle: first\n---\nBody1".into(), "add first".into())
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
        svc.commit_file(
            &format!(".nodes/{node2_uuid}.toml"),
            &node2_config,
            "register node2",
        )
        .unwrap();
    }

    // Add new content after remote's sync point
    driver
        .create_doogat("---\ntitle: second\n---\nBody2".into(), "add second".into())
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
        .create_doogat("---\ntitle: test\n---\nBody".into(), "add".into())
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
        .create_doogat("---\ntitle: first\n---\nBody1".into(), "add first".into())
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
        svc.commit_file(
            &format!(".nodes/{node2_uuid}.toml"),
            &node2_config,
            "register node2",
        )
        .unwrap();
    }

    // Add more content after sync point
    for i in 0..3 {
        driver
            .create_doogat(
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

/// A poisoned service lock should surface as `DdbError` instead of panicking.
#[test]
fn ffi_method_returns_error_on_poisoned_service_lock() {
    let (_tmp, driver) = fresh_driver();

    let poison_handle = {
        let svc_ref = &driver.svc as *const Mutex<DoogatService> as usize;
        std::thread::spawn(move || {
            // SAFETY: the driver lives for the duration of this test; the raw pointer
            // is valid while the join below keeps us in scope.
            let mutex = unsafe { &*(svc_ref as *const Mutex<DoogatService>) };
            let _guard = mutex.lock().unwrap();
            panic!("intentional panic to poison the mutex");
        })
    };
    assert!(poison_handle.join().is_err(), "thread should have panicked");

    let result = driver.list_doogats();
    assert!(result.is_err(), "expected Err on poisoned lock, got Ok");
}

#[test]
fn ffi_singleton_create_exposes_structured_error_context() {
    let (_tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);

    let first_id = driver
        .create_doogat(
            "---\ntitle: Config\ntype: app_config\ntheme: dark\n---\n".into(),
            "add first config".into(),
        )
        .expect("first singleton create should succeed");
    assert!(!first_id.is_empty());

    let err = driver
        .create_doogat(
            "---\ntitle: Config 2\ntype: app_config\ntheme: light\n---\n".into(),
            "add second config".into(),
        )
        .expect_err("second singleton create must reject");

    match err {
        DdbError::Validation {
            code: Some(code),
            context,
            ..
        } => {
            assert_eq!(code, crate::error::codes::SINGLETON_VIOLATION);
            let table = context
                .iter()
                .find(|entry| entry.key == "table")
                .and_then(|entry| entry.value.as_deref());
            let existing_id = context
                .iter()
                .find(|entry| entry.key == "existing_id")
                .and_then(|entry| entry.value.as_deref());
            assert_eq!(table, Some("app_config"));
            assert_eq!(existing_id, Some(first_id.as_str()));
        }
        other => panic!("expected Validation with structured code/context, got {other:?}"),
    }
}
