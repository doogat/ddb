use std::sync::Mutex;

use super::*;
use crate::service::DoogatService;
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

// --- apply_schema FFI tests (PRD 00161) ---
//
// `DoogatDriver::apply_schema(schema_doc, dry_run, allow_destructive)` delegates to
// the core `DoogatService::apply_schema` verb: it diffs a desired-schema YAML doc
// against live typedefs and applies the minimal migration atomically (one
// transaction; a mid-plan failure rolls back). It returns a typed
// `SchemaApplyReportRecord` (NOT a JSON string).

const APPLY_WIDGET_ONE_COLUMN: &str = "\
types:
  - name: widget
    columns:
      - name: label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";

const APPLY_WIDGET_TWO_COLUMNS: &str = "\
types:
  - name: widget
    columns:
      - name: label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: note
        data_type: TEXT
        zone: body
";

const APPLY_GADGET_ONE_COLUMN: &str = "\
types:
  - name: gadget
    columns:
      - name: serial
        data_type: TEXT
        zone: body
";

/// Identical to `APPLY_GADGET_ONE_COLUMN` (same serial/TEXT/body column) but adds a
/// top-level `title_template` on the `gadget` type. A title_template change on an
/// existing type has no DDL path, so the core verb surfaces it as an unsupported
/// change (zero ops) reported in both `report.unsupported` and `report.warnings`.
const APPLY_GADGET_WITH_TITLE_TEMPLATE: &str = "\
types:
  - name: gadget
    title_template: \"{serial}\"
    columns:
      - name: serial
        data_type: TEXT
        zone: body
";

/// dry_run=true returns the plan and mutates nothing: a brand-new type yields a
/// single `create_type` op, `dry_run==true`, `applied==false`, and the type is
/// NOT created (a second dry-run still shows the same create_type op).
#[test]
fn apply_schema_dry_run_returns_plan_without_mutating() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();

    let report = driver
        .apply_schema(APPLY_WIDGET_ONE_COLUMN.into(), true, false)
        .expect("dry-run apply should succeed");

    assert!(report.dry_run, "dry_run flag must be reflected as true");
    assert!(!report.applied, "dry-run must not apply (applied=false)");
    assert_eq!(report.ops.len(), 1, "new type should plan exactly one op");
    assert_eq!(report.ops[0].kind, "create_type");
    assert_eq!(report.ops[0].table, "widget");

    // The op must carry real plan content, not a blanked stub: the create_type op
    // exposes the generated DDL and a human-readable detail. A cheat that blanks
    // `sql`/`detail` after delegating the mutation is caught here.
    let sql_lower = report.ops[0].sql.to_lowercase();
    assert!(
        !report.ops[0].sql.is_empty(),
        "create_type op must expose non-empty sql, got {:?}",
        report.ops[0].sql
    );
    assert!(
        sql_lower.contains("create table"),
        "create_type sql must contain CREATE TABLE, got {:?}",
        report.ops[0].sql
    );
    assert!(
        sql_lower.contains("widget"),
        "create_type sql must reference the widget table, got {:?}",
        report.ops[0].sql
    );
    assert!(
        !report.ops[0].detail.is_empty(),
        "create_type op must expose a non-empty detail, got {:?}",
        report.ops[0].detail
    );

    // Nothing was created: a second dry-run still plans the create_type op,
    // proving the first dry-run did not materialize the type.
    let report2 = driver
        .apply_schema(APPLY_WIDGET_ONE_COLUMN.into(), true, false)
        .expect("second dry-run apply should succeed");
    assert!(!report2.applied);
    assert_eq!(report2.ops.len(), 1);
    assert_eq!(report2.ops[0].kind, "create_type");
    assert_eq!(report2.ops[0].table, "widget");
}

/// dry_run=false creates the declared type and makes it queryable.
/// Uses the distinctive name `gadget` to defeat any hardcoded widget/label impl.
#[test]
fn apply_schema_creates_declared_type_when_not_dry_run() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();

    let report = driver
        .apply_schema(APPLY_GADGET_ONE_COLUMN.into(), false, false)
        .expect("apply should succeed");

    assert!(!report.dry_run);
    assert!(report.applied, "non-dry-run create must report applied=true");
    let create = report
        .ops
        .iter()
        .find(|op| op.kind == "create_type" && op.table == "gadget")
        .expect("plan must contain a create_type op for gadget");
    assert_eq!(create.table, "gadget");

    // The type is now real and queryable through the SQL path.
    assert!(
        driver
            .execute_sql("SELECT serial FROM gadget".into())
            .is_ok(),
        "gadget type should be queryable after apply"
    );
}

/// Re-applying a converged doc is an idempotent no-op: the second apply reports
/// applied=false with an empty ops list.
#[test]
fn apply_schema_reapplying_converged_doc_is_noop() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();

    let first = driver
        .apply_schema(APPLY_WIDGET_ONE_COLUMN.into(), false, false)
        .expect("first apply should succeed");
    assert!(first.applied, "first apply should change the schema");

    let second = driver
        .apply_schema(APPLY_WIDGET_ONE_COLUMN.into(), false, false)
        .expect("re-apply should succeed");
    assert!(
        !second.applied,
        "re-applying a converged doc must not change anything"
    );
    assert!(
        second.ops.is_empty(),
        "converged re-apply must plan zero ops, got {:?}",
        second.ops
    );
}

/// Adding a column to an existing type is non-destructive and applies WITHOUT
/// allow_destructive: yields an `add_column` op and both columns become present.
#[test]
fn apply_schema_adds_column_without_allow_destructive() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();

    driver
        .apply_schema(APPLY_WIDGET_ONE_COLUMN.into(), false, false)
        .expect("initial 1-column apply should succeed");

    let report = driver
        .apply_schema(APPLY_WIDGET_TWO_COLUMNS.into(), false, false)
        .expect("adding a column should succeed without allow_destructive");

    assert!(report.applied, "adding a column must report applied=true");
    let add = report
        .ops
        .iter()
        .find(|op| op.kind == "add_column" && op.table == "widget")
        .expect("plan must contain an add_column op for widget");
    assert!(!add.destructive, "adding a column is not a destructive op");

    // The add_column op must carry real plan content, not a blanked stub. A cheat
    // that blanks `sql`/`detail` on the add_column op (while only create_type's are
    // checked) is caught here.
    let add_sql_lower = add.sql.to_lowercase();
    assert!(
        !add.sql.is_empty(),
        "add_column op must expose non-empty sql, got {:?}",
        add.sql
    );
    assert!(
        add_sql_lower.contains("alter table"),
        "add_column sql must contain ALTER TABLE, got {:?}",
        add.sql
    );
    assert!(
        add_sql_lower.contains("add column"),
        "add_column sql must contain ADD COLUMN, got {:?}",
        add.sql
    );
    assert!(
        add_sql_lower.contains("note"),
        "add_column sql must reference the note column, got {:?}",
        add.sql
    );
    assert!(
        !add.detail.is_empty(),
        "add_column op must expose a non-empty detail, got {:?}",
        add.detail
    );

    // Both columns are now present and queryable.
    assert!(
        driver
            .execute_sql("SELECT label, note FROM widget".into())
            .is_ok(),
        "both label and note columns should exist after add_column"
    );
}

/// A destructive drop without allow_destructive is blocked and mutates nothing.
/// Live widget has label+note; a desired doc with only label implies dropping note.
/// The call must Err with DdbError::Validation { code: SCHEMA_DESTRUCTIVE_BLOCKED },
/// and note must remain present afterward.
#[test]
fn apply_schema_destructive_drop_blocked_without_allow_destructive() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();

    // Establish live widget with both label and note.
    driver
        .apply_schema(APPLY_WIDGET_TWO_COLUMNS.into(), false, false)
        .expect("two-column apply should succeed");

    // Desired doc has only `label`, implying a destructive drop of `note`.
    let err = driver
        .apply_schema(APPLY_WIDGET_ONE_COLUMN.into(), false, false)
        .expect_err("dropping a column without allow_destructive must error");

    match err {
        DdbError::Validation {
            code: Some(code), ..
        } => {
            assert_eq!(code, crate::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED);
        }
        other => panic!("expected Validation SCHEMA_DESTRUCTIVE_BLOCKED, got {other:?}"),
    }

    // The destructive apply must have mutated nothing: note still queryable.
    assert!(
        driver
            .execute_sql("SELECT label, note FROM widget".into())
            .is_ok(),
        "note column must still be present after a blocked destructive apply"
    );
}

/// allow_destructive=true permits the drop: applied=true and afterward the
/// dropped column is gone.
#[test]
fn apply_schema_allow_destructive_permits_drop() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();

    driver
        .apply_schema(APPLY_WIDGET_TWO_COLUMNS.into(), false, false)
        .expect("two-column apply should succeed");

    let report = driver
        .apply_schema(APPLY_WIDGET_ONE_COLUMN.into(), false, true)
        .expect("destructive drop with allow_destructive should succeed");

    assert!(report.applied, "permitted drop must report applied=true");

    // The plan must mark the column drop as destructive. A cheat that hardcodes
    // every op's `destructive = false` is caught here.
    let drop = report
        .ops
        .iter()
        .find(|op| op.kind == "drop_column" && op.destructive)
        .expect("plan must contain a drop_column op flagged destructive");

    // The drop_column op must carry real plan content, not a blanked stub. A cheat
    // that blanks `sql`/`detail` on the drop_column op (while only create_type's are
    // checked) is caught here.
    let drop_sql_lower = drop.sql.to_lowercase();
    assert!(
        drop_sql_lower.contains("alter table"),
        "drop_column sql must contain ALTER TABLE, got {:?}",
        drop.sql
    );
    assert!(
        drop_sql_lower.contains("drop column"),
        "drop_column sql must contain DROP COLUMN, got {:?}",
        drop.sql
    );
    assert!(
        drop_sql_lower.contains("note"),
        "drop_column sql must reference the note column, got {:?}",
        drop.sql
    );
    assert!(
        !drop.detail.is_empty(),
        "drop_column op must expose a non-empty detail, got {:?}",
        drop.detail
    );

    // `note` is gone: selecting it now errors.
    assert!(
        driver
            .execute_sql("SELECT note FROM widget".into())
            .is_err(),
        "note column should be dropped after an allowed destructive apply"
    );
    // `label` survives.
    assert!(
        driver
            .execute_sql("SELECT label FROM widget".into())
            .is_ok(),
        "label column should remain after dropping note"
    );
}

/// An unsupported change (a `title_template` edit on an existing type, which has no
/// DDL path) must surface in BOTH `report.unsupported: Vec<String>` and
/// `report.warnings: Vec<SchemaWarningRecord>` (warning code
/// `SCHEMA_UNSUPPORTED_CHANGE`). A cheat that blanks `warnings`/`unsupported` after
/// delegating the mutation is caught here. Loose assertions (non-empty + `any(...)`)
/// keep this stable against incidental extra diffs.
#[test]
fn apply_schema_surfaces_unsupported_change_as_warning() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().unwrap();

    // Establish a live `gadget` type with no title_template.
    driver
        .apply_schema(APPLY_GADGET_ONE_COLUMN.into(), false, false)
        .expect("initial gadget apply should succeed");

    // Dry-run a doc that only adds a title_template: an unsupported change. dry_run
    // keeps this read-only so the assertions describe the plan, not a mutation.
    let report = driver
        .apply_schema(APPLY_GADGET_WITH_TITLE_TEMPLATE.into(), true, false)
        .expect("dry-run apply with unsupported change should succeed");

    assert!(
        !report.unsupported.is_empty(),
        "unsupported change must populate report.unsupported, got {:?}",
        report.unsupported
    );
    assert!(
        report
            .unsupported
            .iter()
            .any(|entry| entry.contains("title_template")),
        "an unsupported entry must mention title_template, got {:?}",
        report.unsupported
    );

    assert!(
        !report.warnings.is_empty(),
        "unsupported change must populate report.warnings, got {:?}",
        report.warnings
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == crate::app_contract::SCHEMA_UNSUPPORTED_CHANGE),
        "a warning must carry code SCHEMA_UNSUPPORTED_CHANGE, got {:?}",
        report.warnings
    );

    // The warning must carry a real message, not a blanked stub. A cheat that checks
    // only `code` and blanks the warning `message` is caught here.
    assert!(
        report.warnings.iter().any(|w| w.code
            == crate::app_contract::SCHEMA_UNSUPPORTED_CHANGE
            && w.message.to_lowercase().contains("title_template")),
        "the SCHEMA_UNSUPPORTED_CHANGE warning must carry a non-empty message mentioning title_template, got {:?}",
        report.warnings
    );
}

// --- reindex() FFI warnings tests ---
//
// `Index::rebuild` already collects per-file parse failures into the core
// `RebuildReport.warnings: Vec<ConsistencyWarning>`. `DoogatDriver::reindex()` must
// surface the FULL per-file list as `RebuildWarningRecord { code, message }` entries
// (unlike the AppOutput facades, which summarize to at most one warning).

const VALID_DOOGAT: &str = "---
title: Valid Doogat
---
Body
";

const POISON_DOOGAT_UNCLOSED_BRACKET: &str = "---
title: [unclosed
---
Body
";

const POISON_DOOGAT_TAB_INDENT: &str = "---
title: broken
\tbad: value
---
Body
";

fn commit_doogat(driver: &DoogatDriver, path: &str, content: &str) {
    let svc = driver.svc.lock().unwrap();
    svc.repo.commit_file(path, content, "add doogat").unwrap();
}

#[test]
fn reindex_clean_repo_yields_empty_warnings() {
    let (_tmp, driver) = fresh_driver();

    let report = driver.reindex().unwrap();

    assert!(
        report.warnings.is_empty(),
        "clean repo should yield an empty warnings list, got {} entries",
        report.warnings.len()
    );
}

#[test]
fn reindex_malformed_yaml_file_reports_stable_code_and_path() {
    let (_tmp, driver) = fresh_driver();
    let poison_path = "ddb/20260101000001.md";
    commit_doogat(&driver, poison_path, POISON_DOOGAT_UNCLOSED_BRACKET);

    let report = driver.reindex().unwrap();

    let warning = report
        .warnings
        .iter()
        .find(|w| w.message.contains(poison_path))
        .expect("a warning naming the poison file path should be present");
    assert_eq!(warning.code, "MALFORMED_YAML");
}

#[test]
fn reindex_two_poison_files_returns_full_per_file_list() {
    let (_tmp, driver) = fresh_driver();
    let poison_path_a = "ddb/20260101000002.md";
    let poison_path_b = "ddb/20260101000003.md";
    commit_doogat(&driver, poison_path_a, POISON_DOOGAT_UNCLOSED_BRACKET);
    commit_doogat(&driver, poison_path_b, POISON_DOOGAT_TAB_INDENT);

    let report = driver.reindex().unwrap();
    let messages: Vec<&str> = report.warnings.iter().map(|w| w.message.as_str()).collect();

    // Not asserted as an exact count: `Index::rebuild` reports each unparseable file
    // from two separate phases (parallel_parse AND collect_consistency_warnings), so
    // two poison files yield four entries today, not two. That duplication is
    // pre-existing core behavior and out of scope here. What this test binds is the
    // FFI-specific contract: the list is the full per-file one (more than the
    // at-most-one entry the summarizing AppOutput path would give), not a summary.
    assert!(
        report.warnings.len() > 1,
        "the FFI warnings list must carry more than the single summarized entry the AppOutput path would give, got {} entries: {messages:?}",
        report.warnings.len()
    );
    assert!(
        messages.iter().any(|m| m.contains(poison_path_a)),
        "warnings must name {poison_path_a}, got {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains(poison_path_b)),
        "warnings must name {poison_path_b}, got {messages:?}"
    );
    assert!(
        messages.iter().all(|m| !m.contains("more, see ddb doctor")),
        "FFI warnings must not carry the AppOutput summarizing suffix, got {messages:?}"
    );
}

#[test]
fn reindex_succeeds_and_indexes_healthy_doogat_despite_poison_file() {
    let (_tmp, driver) = fresh_driver();
    let poison_path = "ddb/20260101000004.md";
    commit_doogat(&driver, poison_path, POISON_DOOGAT_UNCLOSED_BRACKET);
    commit_doogat(&driver, "ddb/20260101000005.md", VALID_DOOGAT);

    let report = driver
        .reindex()
        .expect("reindex must succeed (Ok) even with a poison file present");

    assert_eq!(
        report.indexed, 1,
        "the healthy doogat must still be indexed, got indexed={}",
        report.indexed
    );
    assert!(
        !report.warnings.is_empty(),
        "the poison file must still be surfaced as a warning, not silently dropped"
    );
}
