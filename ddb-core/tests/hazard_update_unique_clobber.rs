//! Hazard H1 — update has no UNIQUE pre-check, and materialization uses
//! `INSERT OR REPLACE` under a UNIQUE index.
//!
//! Evidence (static read, never executed before this file existed):
//! - `ddb-core/src/service/update.rs:74-111` — the `update_doogat_parsed`
//!   write closure runs `check_singleton_update_constraint` and
//!   `validate_fields_with_schemas`, but never `check_unique_constraints`.
//!   Same gap in `ddb-core/src/service/update.rs:286-343` (`prepare_update`,
//!   the batch leg). The only `check_unique_constraints` callers are
//!   `service/create.rs:465` and `service/batch.rs:200`.
//! - `ddb-core/src/indexer/materialize.rs:923-928` — `materialize_single`
//!   writes the typed table with `INSERT OR REPLACE INTO "<table>"`.
//! - `ddb-core/src/sql_engine/ddl.rs:402-421` — every `unique_together`
//!   group (a column-level `UNIQUE` included) becomes a
//!   `CREATE UNIQUE INDEX` on the materialized table.
//!
//! Under SQLite, `INSERT OR REPLACE` resolves a UNIQUE-index conflict by
//! DELETING the conflicting row. So updating B's unique column to A's value
//! lands in git and silently evicts A from the typed table — A survives in
//! git and in `doogats`, but disappears from every typed read path.
//!
//! These tests assert the SAFE behavior: either the update is rejected with
//! `UNIQUE_VIOLATION`, or it succeeds and BOTH rows remain in the typed
//! table with A's value untouched. A failure here means the hazard is real
//! and the assertion message names which surface fired it.

use ddb_core::app_contract::{CreateCommand, UnregisteredTypePolicy, UpdateCommand};
use ddb_core::error::{codes, DoogatError};
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{BatchUpdateInput, ConflictAction, Value};
use std::collections::BTreeMap;

/// Registered typedef with one UNIQUE column, materialized into table `link`.
fn init_service_with_unique_typedef(dir: &std::path::Path) -> DoogatService {
    let mut svc = DoogatService::init(dir).expect("init repo");
    svc.reindex().expect("initial reindex");
    svc.execute_sql("CREATE TABLE link (slug VARCHAR(64) UNIQUE)")
        .expect("create link typedef with UNIQUE slug");
    svc
}

fn slug_fields(slug: &str) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    fields.insert("slug".to_string(), Value::String(slug.to_string()));
    fields
}

fn create_link(svc: &DoogatService, title: &str, slug: &str) -> String {
    let output = svc
        .create(CreateCommand {
            title: Some(title.to_string()),
            body: None,
            tags: vec![],
            doogat_type: Some("link".to_string()),
            fields: slug_fields(slug),
            on_conflict: ConflictAction::Error,
            unregistered_type_policy: UnregisteredTypePolicy::Strict,
        })
        .unwrap_or_else(|e| panic!("baseline create of {title} must succeed, got: {e:?}"));
    output
        .value
        .meta
        .id
        .unwrap_or_else(|| panic!("created doogat {title} must carry an id"))
        .0
}

fn typed_row_count(svc: &mut DoogatService) -> usize {
    match svc
        .execute_sql("SELECT id FROM link")
        .expect("SELECT against the materialized link table must succeed")
    {
        SqlResult::Rows { rows, .. } => rows.len(),
        other => panic!("expected SELECT to return rows, got: {other:?}"),
    }
}

/// The safe contract, whichever way it is honored: reject with
/// `UNIQUE_VIOLATION`, or keep both typed rows and leave A's value alone.
fn assert_unique_collision_is_safe(
    svc: &mut DoogatService,
    outcome: Result<(), DoogatError>,
    id_a: &str,
    surface: &str,
) {
    match outcome {
        Err(DoogatError::Structured { code, .. }) if code == codes::UNIQUE_VIOLATION => {}
        Err(other) => panic!(
            "H1 ({surface}): a UNIQUE collision on update must be rejected with \
             UNIQUE_VIOLATION, got a different error: {other:?}"
        ),
        Ok(()) => {
            let count = typed_row_count(svc);
            assert_eq!(
                count, 2,
                "H1 FIRED ({surface}): update accepted a UNIQUE collision and the \
                 materialized `link` table now holds {count} row(s) instead of 2 — \
                 INSERT OR REPLACE evicted the other row from every typed read path \
                 while git still holds it"
            );

            let parsed_a = svc
                .get_doogat_parsed(id_a)
                .expect("row A must still be readable from git after the collision update");
            assert_eq!(
                parsed_a.meta.extra.get("slug"),
                Some(&Value::String("x".to_string())),
                "H1 FIRED ({surface}): row A's stored slug changed after updating a \
                 DIFFERENT doogat; expected slug `x`, got {:?}",
                parsed_a.meta.extra.get("slug")
            );
        }
    }
}

#[test]
fn update_rejects_unique_collision_or_keeps_both_typed_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service_with_unique_typedef(tmp.path());

    let id_a = create_link(&svc, "Row A", "x");
    let id_b = create_link(&svc, "Row B", "y");
    assert_eq!(typed_row_count(&mut svc), 2, "both rows must materialize");

    let outcome = svc
        .update(UpdateCommand {
            id: id_b,
            title: None,
            tags: None,
            doogat_type: None,
            body: None,
            fields: slug_fields("x"),
            unset_fields: vec![],
        })
        .map(|_| ());

    assert_unique_collision_is_safe(&mut svc, outcome, &id_a, "DoogatService::update");
}

#[test]
fn batch_update_rejects_unique_collision_or_keeps_both_typed_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service_with_unique_typedef(tmp.path());

    let id_a = create_link(&svc, "Row A", "x");
    let id_b = create_link(&svc, "Row B", "y");
    assert_eq!(typed_row_count(&mut svc), 2, "both rows must materialize");

    let outcome = svc
        .batch_update(&[BatchUpdateInput {
            id: id_b,
            title: None,
            body: None,
            tags: None,
            doogat_type: None,
            fields: Some(slug_fields("x")),
            unset_fields: None,
        }])
        .map(|_| ());

    assert_unique_collision_is_safe(&mut svc, outcome, &id_a, "DoogatService::batch_update");
}

/// Two updates in ONE batch that land on the same UNIQUE value pass the
/// per-item check individually (neither row is materialized yet), so the
/// batch lane must also track collisions inside the batch.
#[test]
fn batch_update_rejects_intra_batch_unique_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service_with_unique_typedef(tmp.path());

    let id_a = create_link(&svc, "Row A", "x");
    let id_b = create_link(&svc, "Row B", "y");
    assert_eq!(typed_row_count(&mut svc), 2, "both rows must materialize");

    let to_z = |id: String| BatchUpdateInput {
        id,
        title: None,
        body: None,
        tags: None,
        doogat_type: None,
        fields: Some(slug_fields("z")),
        unset_fields: None,
    };
    let outcome = svc.batch_update(&[to_z(id_a), to_z(id_b)]);

    match outcome {
        Err(DoogatError::Structured { code, .. }) if code == codes::UNIQUE_VIOLATION => {}
        other => panic!(
            "H1 (intra-batch): two updates in one batch racing the same UNIQUE value must be \
             rejected with UNIQUE_VIOLATION, got: {other:?}"
        ),
    }
    assert_eq!(
        typed_row_count(&mut svc),
        2,
        "a rejected batch must leave both typed rows in place"
    );
}
