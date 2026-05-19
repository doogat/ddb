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

/// PRD 00139: `create_doogat_with_extra` (the typed CLI/FFI create path)
/// must reject a second SINGLETON row with the structured
/// `SINGLETON_VIOLATION` error — not leak a raw SQL UNIQUE-constraint error.
///
/// Hardening (PRD 00139 review): the typedef here is `site_settings` (NOT
/// `app_config`, which the sibling raw test uses), and the two rejected
/// creates carry two DIFFERENT titles (`another`, `yet-another`), neither
/// of which is `second`. Together this makes the test impossible to satisfy
/// by hardcoding `(type, title)` literals — a `type`-keyed branch can't fire
/// on `site_settings`, and a `title`-keyed branch would let at least one of
/// the two distinct titles slip through.
#[test]
fn create_doogat_with_extra_rejects_second_singleton_row_with_structured_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    svc.execute_sql("CREATE TABLE site_settings (theme TEXT) SINGLETON")
        .unwrap();

    let mut first_extra = BTreeMap::new();
    first_extra.insert("theme".to_string(), Value::String("dark".to_string()));
    let first = svc
        .create_doogat_with_extra("first", &[], Some("site_settings"), "", first_extra)
        .expect("first singleton row must be created");
    let first_id = first
        .meta
        .id
        .expect("created singleton row must carry an id")
        .0;

    let expected_message =
        format!("SINGLETON constraint violated: site_settings already holds row {first_id}");

    // Two further creates with two DISTINCT titles, neither `second`.
    // BOTH must be rejected with the identical structured error — a
    // title-keyed hardcode would let one through.
    for title in ["another", "yet-another"] {
        let mut extra = BTreeMap::new();
        extra.insert("theme".to_string(), Value::String("light".to_string()));
        let err = svc
            .create_doogat_with_extra(title, &[], Some("site_settings"), "", extra)
            .expect_err("create_doogat_with_extra must reject the second singleton row");
        let structured = extract_singleton_structured(err, "create_doogat_with_extra");

        assert_eq!(structured.code, codes::SINGLETON_VIOLATION);
        assert_eq!(structured.table, "site_settings");
        assert_eq!(structured.existing_id, first_id);
        assert_eq!(structured.message, expected_message);
    }
}

/// PRD 00139: regression coverage — `create_doogat_raw` must also reject a
/// second SINGLETON row with the same structured `SINGLETON_VIOLATION`.
#[test]
fn create_doogat_raw_rejects_second_singleton_row_with_structured_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    svc.execute_sql("CREATE TABLE app_config (theme TEXT) SINGLETON")
        .unwrap();

    let first_id = svc
        .create_doogat_raw(
            "---\ntype: app_config\ntitle: first\ntheme: dark\n---\nbody",
            "add first singleton row",
        )
        .expect("first singleton row must be created");

    let err = svc
        .create_doogat_raw(
            "---\ntype: app_config\ntitle: second\ntheme: light\n---\nbody",
            "add second singleton row",
        )
        .expect_err("create_doogat_raw must reject the second singleton row");
    let structured = extract_singleton_structured(err, "create_doogat_raw");

    assert_eq!(structured.code, codes::SINGLETON_VIOLATION);
    assert_eq!(structured.table, "app_config");
    assert_eq!(structured.existing_id, first_id);
    assert_eq!(
        structured.message,
        format!(
            "SINGLETON constraint violated: {} already holds row {}",
            structured.table, structured.existing_id
        )
    );
}

/// PRD 00139: a NON-singleton typedef must be unaffected — two successive
/// `create_doogat_with_extra` calls into a non-singleton typedef both
/// succeed (no false SINGLETON rejection).
///
/// Hardening (PRD 00139 review): the non-singleton control typedef
/// `plain_config` is now STRUCTURALLY IDENTICAL to the singleton typedefs
/// `site_settings`/`app_config` — same table-name shape, and the SAME
/// `theme TEXT` column — differing ONLY in the absence of the `SINGLETON`
/// keyword. The earlier `notes (body TEXT)` control diverged in both name
/// and column name, so any column-name or column-shape proxy (e.g.
/// `columns.any(|c| c.name == "theme")`) could masquerade as real
/// enforcement. With byte-identical column shapes, no such proxy can tell
/// `plain_config` from a singleton typedef — only the `singleton` flag on
/// the typedef does, so a correct implementation MUST consult it to let
/// these two rows through.
#[test]
fn create_doogat_with_extra_allows_two_rows_in_non_singleton_typedef() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    svc.execute_sql("CREATE TABLE plain_config (theme TEXT)")
        .unwrap();

    let mut first_extra = BTreeMap::new();
    first_extra.insert("theme".to_string(), Value::String("one".to_string()));
    let first = svc.create_doogat_with_extra("first", &[], Some("plain_config"), "", first_extra);
    assert!(
        first.is_ok(),
        "first non-singleton row must succeed, got: {first:?}"
    );

    let mut second_extra = BTreeMap::new();
    second_extra.insert("theme".to_string(), Value::String("two".to_string()));
    let second =
        svc.create_doogat_with_extra("second", &[], Some("plain_config"), "", second_extra);
    assert!(
        second.is_ok(),
        "second non-singleton row must succeed, got: {second:?}"
    );
}

/// Read the single `theme` cell out of `SELECT theme FROM <table>`,
/// asserting there is exactly `expected_rows` rows materialized.
fn read_theme(
    svc: &mut DoogatService<impl ddb_core::traits::GitBackend>,
    table: &str,
    expected_rows: usize,
) -> Option<String> {
    match svc
        .execute_sql(&format!("SELECT theme FROM {table}"))
        .expect("SELECT must succeed")
    {
        ddb_core::sql_engine::SqlResult::Rows { rows, .. } => {
            assert_eq!(
                rows.len(),
                expected_rows,
                "{table} must hold exactly {expected_rows} row(s), got {}",
                rows.len()
            );
            rows.into_iter().next().map(|mut r| r.remove(0))
        }
        other => panic!("SELECT must produce Rows, got: {other:?}"),
    }
}

/// AC1: `upsert_singleton` on an EMPTY SINGLETON typedef creates the row,
/// returns `created == true` with a non-empty `id`, and the supplied field
/// value is materialized.
///
/// Hardening (PRD 00139 review): this test loops over TWO distinct singleton
/// typedef names — `site_settings` and `app_config` — symmetric to AC3's
/// three-way non-singleton loop. A wrong implementation with a single-name
/// accept-list (e.g. `if doogat_type != "site_settings" { return Err(...) }`)
/// passes on the `site_settings` iteration but fails on `app_config`; only
/// reading the typedef's actual `singleton` flag satisfies both.
#[test]
fn upsert_singleton_creates_row_when_typedef_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    for table in ["site_settings", "app_config"] {
        svc.execute_sql(&format!("CREATE TABLE {table} (theme TEXT) SINGLETON"))
            .unwrap();

        let fields = BTreeMap::from([("theme".to_string(), Value::String("dark".to_string()))]);
        let outcome = svc
            .upsert_singleton(table, fields)
            .expect("upsert into empty singleton typedef must succeed");

        assert!(
            outcome.created,
            "first upsert into an empty typedef `{table}` must report created == true"
        );
        assert!(
            !outcome.id.is_empty(),
            "created row in `{table}` must carry a non-empty id"
        );

        assert_eq!(
            read_theme(&mut svc, table, 1).as_deref(),
            Some("dark"),
            "supplied field value must be materialized in `{table}`"
        );
    }
}

/// AC2: a SECOND `upsert_singleton` on the now-populated typedef updates the
/// existing row in place: `created == false`, the SAME id as the first call,
/// the typedef still holds exactly ONE row, and the new field value is
/// reflected. This rejects an implementation that always inserts (duplicate
/// row) or always reports `created == true`.
///
/// Hardening (PRD 00139 review): this test loops over TWO distinct singleton
/// typedef names — `site_settings` and `app_config` — symmetric to AC3's
/// three-way non-singleton loop. A wrong implementation with a single-name
/// accept-list (e.g. `if doogat_type != "site_settings" { return Err(...) }`)
/// passes on the `site_settings` iteration but fails on `app_config`; only
/// reading the typedef's actual `singleton` flag satisfies both.
#[test]
fn upsert_singleton_updates_existing_row_and_returns_created_false() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    for table in ["site_settings", "app_config"] {
        svc.execute_sql(&format!("CREATE TABLE {table} (theme TEXT) SINGLETON"))
            .unwrap();

        let first = svc
            .upsert_singleton(
                table,
                BTreeMap::from([("theme".to_string(), Value::String("dark".to_string()))]),
            )
            .expect("first upsert must succeed");
        assert!(
            first.created,
            "first upsert into `{table}` must create the row"
        );

        let second = svc
            .upsert_singleton(
                table,
                BTreeMap::from([("theme".to_string(), Value::String("light".to_string()))]),
            )
            .expect("second upsert must succeed");

        assert!(
            !second.created,
            "second upsert into `{table}` must update the existing row, not create one"
        );
        assert_eq!(
            second.id, first.id,
            "second upsert into `{table}` must target the SAME row id as the first"
        );

        // Still exactly one row, and it carries the second call's value.
        assert_eq!(
            read_theme(&mut svc, table, 1).as_deref(),
            Some("light"),
            "second upsert's field value must be reflected in the single row of `{table}`"
        );
    }
}

/// AC3: `upsert_singleton` on a NON-singleton typedef must return `Err`.
///
/// Hardening (PRD 00139 review): each non-singleton typedef here is
/// STRUCTURALLY IDENTICAL to the singleton typedef `site_settings` exercised
/// by AC1/AC2 — same `theme TEXT` column shape — differing ONLY in the
/// absence of the `SINGLETON` keyword. No type-name or column-shape proxy
/// can distinguish them from `site_settings`, so a correct implementation
/// MUST consult the typedef's actual `singleton` flag to reject this call.
///
/// Parametrization (this review cycle): the test loops over THREE distinct
/// non-singleton typedef names — `plain_config`, `misc_config`, `user_prefs`
/// — mirroring how `upsert_singleton_updates_existing_row_and_returns_created_false`
/// loops over two distinct titles. A wrong implementation that hardcodes a
/// single literal type name (e.g. `if doogat_type == "plain_config"`) would
/// reject only that one name and let `misc_config` and `user_prefs` slip
/// through, failing the test. Only reading the typedef's `singleton` flag
/// rejects all three.
#[test]
fn upsert_singleton_rejects_non_singleton_typedef() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    for table in ["plain_config", "misc_config", "user_prefs"] {
        svc.execute_sql(&format!("CREATE TABLE {table} (theme TEXT)"))
            .unwrap();
        let result = svc.upsert_singleton(
            table,
            BTreeMap::from([("theme".to_string(), Value::String("dark".to_string()))]),
        );
        assert!(
            result.is_err(),
            "upsert_singleton on non-singleton typedef `{table}` must return Err, got: {result:?}"
        );
    }
}
