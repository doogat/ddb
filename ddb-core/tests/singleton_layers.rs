//! PRD 00139 §3 / T23: SINGLETON enforcement parity across all three
//! layers.
//!
//! - Layer 1 — `service::validation::check_singleton_constraint`, fired
//!   from `service::DoogatService::batch_create`.
//! - Layer 2 — `sql_engine::dml::handle_insert` SINGLETON pre-check, fired
//!   from `service::DoogatService::execute_sql("INSERT INTO ...")`.
//! - Layer 3 — materializer-side `materialize_row` SINGLETON pre-check +
//!   `<table>_singleton_lock` UNIQUE index, fired from
//!   `service::DoogatService::reindex()` after a direct git write bypasses
//!   the higher write paths.
//!
//! This integration test pins the contract that all three enforcement
//! layers surface byte-identical structured errors (`code`, `message`,
//! `table`, and `existing_id`) when they reject the same second row.

use std::collections::BTreeMap;

use ddb_core::error::{codes, DoogatError, ErrorValue};
use ddb_core::service::DoogatService;
use ddb_core::types::{BatchCreateInput, ConflictAction, Value};

#[derive(Debug, PartialEq, Eq)]
struct SingletonStructuredError {
    code: &'static str,
    message: String,
    table: String,
    existing_id: String,
}

fn context_string(context: &[(String, ErrorValue)], key: &str, layer: &str) -> String {
    match context.iter().find(|(candidate, _)| candidate == key) {
        Some((_, ErrorValue::String(value))) => value.clone(),
        Some((_, value)) => panic!("{layer} ctx `{key}` must be a string, got: {value:?}"),
        None => panic!("{layer} ctx must include `{key}`"),
    }
}

fn extract_singleton_structured(err: DoogatError, layer: &str) -> SingletonStructuredError {
    match err {
        DoogatError::Structured {
            code,
            message,
            context,
        } => SingletonStructuredError {
            code,
            table: context_string(&context, "table", layer),
            existing_id: context_string(&context, "existing_id", layer),
            message,
        },
        other => panic!("{layer} must produce Structured, got: {other:?}"),
    }
}

#[test]
fn singleton_layers_one_two_and_three_share_structured_error_prd_00139() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    // Install a SINGLETON typedef end-to-end via the public DDL surface.
    svc.execute_sql("CREATE TABLE app_config (theme TEXT) SINGLETON")
        .unwrap();
    // Seed one row via the SQL DML path so all three layers see the same
    // already-materialized blocker row.
    svc.execute_sql("INSERT INTO app_config (title, theme) VALUES ('first', 'dark')")
        .unwrap();

    // Layer 1: validator path (service::batch_create → check_singleton_constraint).
    let mut fields = BTreeMap::new();
    fields.insert("theme".to_string(), Value::String("light".to_string()));
    let err_layer1 = svc
        .batch_create(&[BatchCreateInput {
            title: Some("Layer1".to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("app_config".to_string()),
            fields,
            on_conflict: ConflictAction::Error,
        }])
        .expect_err("layer 1 (validator) must reject the second insert");

    // Layer 2: SQL DML pre-check (raw `ddb query INSERT`).
    let err_layer2 = svc
        .execute_sql("INSERT INTO app_config (title, theme) VALUES ('Layer2', 'auto')")
        .expect_err("layer 2 (DML pre-check) must reject the second insert");

    let layer1 = extract_singleton_structured(err_layer1, "layer 1");
    let layer2 = extract_singleton_structured(err_layer2, "layer 2");

    assert_eq!(layer1.code, codes::SINGLETON_VIOLATION);
    assert_eq!(layer2.code, codes::SINGLETON_VIOLATION);
    assert_eq!(layer1.table, "app_config");
    assert_eq!(layer1, layer2);

    let second_row_id = format!(
        "{:014}",
        layer1
            .existing_id
            .parse::<u64>()
            .expect("existing singleton row id must be numeric")
            + 1
    );
    let second_row_path = format!("ddb/{second_row_id}.md");
    let second_row = format!(
        "\
---
id: {second_row_id}
title: Layer3
type: app_config
theme: solarized
---
Direct git write to trigger materializer enforcement
"
    );

    // Layer 3: bypass validator + SQL DML, land a second typed row in git,
    // then force a full rematerialization.
    svc.commit_file(&second_row_path, &second_row, "add direct singleton row")
        .unwrap();
    let err_layer3 = svc
        .reindex()
        .expect_err("layer 3 (materializer) must reject the second row during reindex");
    let layer3 = extract_singleton_structured(err_layer3, "layer 3");

    assert_eq!(layer3.code, codes::SINGLETON_VIOLATION);
    // PRD 00139 cycle-3 #9: this byte-identical equality assertion (including
    // `existing_id`) depends on a deterministic ordering invariant:
    //
    // 1. `list_doogats()` and `populate_materialized_table[_from]()` both
    //    process doogats in alphabetical-path order. The path ordering is
    //    pinned by `SELECT ... ORDER BY path ASC` (PRD 00139 cycle-3 #5)
    //    and by `list_doogats()`'s `BTreeMap`-backed iteration.
    // 2. With `second_row_id = existing_id + 1`, the alphabetically-earlier
    //    row (the "first" existing row, id) is what each layer reports as
    //    `existing_id` in SINGLETON_VIOLATION.
    //
    // If a future refactor of `list_doogats` or `populate_materialized_table`
    // ever returns rows in non-alphabetical order, this assertion will
    // become non-deterministic — re-pin the invariant before relaxing
    // either function.
    assert_eq!(layer1, layer3);

    let expected_message = format!(
        "SINGLETON constraint violated: {} already holds row {}",
        layer1.table, layer1.existing_id
    );
    assert_eq!(layer1.message, expected_message);
    assert_eq!(layer2.message, expected_message);
    assert_eq!(layer3.message, expected_message);
}

#[test]
fn execute_sql_refreshes_stale_singleton_index_before_dml_precheck_prd_00139() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    svc.execute_sql("CREATE TABLE app_config (theme TEXT) SINGLETON")
        .unwrap();

    let existing_id = "20260510121500";
    let existing_path = format!("ddb/{existing_id}.md");
    let existing_row = format!(
        "\
---
id: {existing_id}
title: ExternalRow
type: app_config
theme: dark
---
Direct git write that bypasses service-side INSERT
"
    );
    svc.commit_file(
        &existing_path,
        &existing_row,
        "add singleton row outside service",
    )
    .unwrap();

    assert!(
        svc.is_index_stale().unwrap(),
        "direct git write must leave the materialized index stale until ensure_fresh runs"
    );

    let err = svc
        .execute_sql("INSERT INTO app_config (title, theme) VALUES ('second', 'light')")
        .expect_err("stale-index second insert must reject after execute_sql refreshes");
    let structured = extract_singleton_structured(err, "execute_sql freshness");

    assert_eq!(structured.code, codes::SINGLETON_VIOLATION);
    assert_eq!(structured.table, "app_config");
    assert_eq!(structured.existing_id, existing_id);
    assert_eq!(
        structured.message,
        format!(
            "SINGLETON constraint violated: {} already holds row {}",
            structured.table, structured.existing_id
        )
    );
}
