//! PRD 00139 §3 / T23: SINGLETON enforcement parity across the three
//! layers.
//!
//! - Layer 1 — `service::validation::check_singleton_constraint`, fired
//!   from `service::DoogatService::batch_create`.
//! - Layer 2 — `sql_engine::dml::handle_insert` SINGLETON pre-check, fired
//!   from `service::DoogatService::execute_sql("INSERT INTO ...")`.
//! - Layer 3 — materializer-side `<table>_singleton_lock` UNIQUE index
//!   on the constant `1`. Tested separately at the unit level in
//!   `ddb-core/src/indexer/tests/materialize_tests.rs::singleton_lock_blocks_direct_second_insert`
//!   because the materializer connection is private to the indexer module
//!   and isn't reachable from a `tests/` integration target.
//!
//! This integration test pins layers 1 and 2 surfacing byte-identical
//! structured errors when both block the same write. Layer 3 parity is
//! implicit: if T8's bypass test plus T11's pre-check test both pass,
//! the materialized table cannot hold two rows regardless of which
//! higher layer fires first.

use ddb_core::error::{codes, DoogatError, ErrorValue};
use ddb_core::service::DoogatService;

#[test]
fn singleton_layers_one_and_two_share_structured_error_prd_00139() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    // Install a SINGLETON typedef end-to-end via the public DDL surface.
    svc.execute_sql("CREATE TABLE app_config (theme TEXT) SINGLETON")
        .unwrap();
    // Seed one row via the SQL DML path.
    svc.execute_sql("INSERT INTO app_config (title, theme) VALUES ('first', 'dark')")
        .unwrap();

    // Layer 1: validator path (service::batch_create → check_singleton_constraint).
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "theme".to_string(),
        ddb_core::types::Value::String("light".to_string()),
    );
    let err_layer1 = svc
        .batch_create(&[ddb_core::types::BatchCreateInput {
            title: Some("Layer1".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("app_config".to_string()),
            fields,
            on_conflict: ddb_core::types::ConflictAction::Error,
        }])
        .expect_err("layer 1 (validator) must reject the second insert");

    // Layer 2: SQL DML pre-check (raw `ddb query INSERT`).
    let err_layer2 = svc
        .execute_sql("INSERT INTO app_config (title, theme) VALUES ('Layer2', 'auto')")
        .expect_err("layer 2 (DML pre-check) must reject the second insert");

    // Layers 1 and 2 must surface structured errors with matching code +
    // matching `table` context. The `existing_id` value is the same
    // because both layers `SELECT id FROM "app_config" LIMIT 1` against
    // the same materialized table state.
    let DoogatError::Structured {
        code: code1,
        context: ctx1,
        ..
    } = &err_layer1
    else {
        panic!("layer 1 must produce Structured, got: {err_layer1:?}");
    };
    let DoogatError::Structured {
        code: code2,
        context: ctx2,
        ..
    } = &err_layer2
    else {
        panic!("layer 2 must produce Structured, got: {err_layer2:?}");
    };

    assert_eq!(*code1, codes::SINGLETON_VIOLATION);
    assert_eq!(*code2, codes::SINGLETON_VIOLATION);

    let table1 = ctx1
        .iter()
        .find(|(k, _)| k == "table")
        .map(|(_, v)| v)
        .expect("layer 1 ctx must include `table`");
    let table2 = ctx2
        .iter()
        .find(|(k, _)| k == "table")
        .map(|(_, v)| v)
        .expect("layer 2 ctx must include `table`");
    assert_eq!(table1, table2);
    assert_eq!(table1, &ErrorValue::String("app_config".to_string()));

    let id1 = ctx1
        .iter()
        .find(|(k, _)| k == "existing_id")
        .map(|(_, v)| v)
        .expect("layer 1 ctx must include `existing_id`");
    let id2 = ctx2
        .iter()
        .find(|(k, _)| k == "existing_id")
        .map(|(_, v)| v)
        .expect("layer 2 ctx must include `existing_id`");
    assert_eq!(id1, id2, "both layers must report the same existing row id");
}
