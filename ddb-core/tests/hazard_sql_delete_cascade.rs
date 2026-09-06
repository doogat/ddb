//! Hazard H3: SQL `DELETE FROM parent` runs no `ON DELETE CASCADE` walk.
//!
//! Evidence: `ddb-core/src/sql_engine/dml.rs:681-712` (`delete_single_row`)
//! and `:727` (`delete_bulk_rows`) call only `check_restrict_blocks_delete`;
//! `collect_cascade_children` is reached solely from
//! `ddb-core/src/service/delete.rs:49` (`build_cascade_delete_plan`). So
//! `DELETE FROM parent WHERE id = 'P'` over SQL may commit with CASCADE
//! children left alive in git and in their typed table, while
//! `DoogatService::delete_doogat` (the `ddb delete` path) removes them.
//! `docs/src/technical/sql-engine.md:440-457` documents CASCADE under the
//! `DELETE FROM` section, so both paths must agree.
//!
//! Safe behavior pinned here: a successful SQL DELETE of a CASCADE parent
//! leaves no child in git and no child row in the typed table; a refusal is
//! acceptable only as a structured error naming the child table that
//! changes nothing. The second test is the control proving the service path
//! cascades. First test failing while the control passes means H3 fired.

use ddb_core::error::DoogatError;
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;

fn id_from_insert(svc: &mut DoogatService, sql: &str) -> String {
    match svc.execute_sql(sql).unwrap() {
        SqlResult::Ok(id) => id,
        other => panic!("expected SqlResult::Ok with id, got {other:?}"),
    }
}

/// Register `link` (parent) and `membership` (child, CASCADE on `link`),
/// insert one of each, and return `(parent_id, child_id)`.
fn seed_cascade_pair(svc: &mut DoogatService) -> (String, String) {
    svc.execute_sql("CREATE TABLE link (title TEXT)").unwrap();
    svc.execute_sql(
        "CREATE TABLE membership (title TEXT, link VARCHAR(255) NOT NULL REFERENCES link(id) ON DELETE CASCADE)",
    )
    .unwrap();
    let parent_id = id_from_insert(svc, "INSERT INTO link (title) VALUES ('Parent')");
    let child_id = id_from_insert(
        svc,
        &format!("INSERT INTO membership (title, link) VALUES ('Child', '{parent_id}')"),
    );
    (parent_id, child_id)
}

fn child_rows(svc: &mut DoogatService, child_id: &str) -> Vec<Vec<String>> {
    match svc
        .execute_sql(&format!("SELECT id FROM membership WHERE id = '{child_id}'"))
        .unwrap()
    {
        SqlResult::Rows { rows, .. } => rows,
        other => panic!("expected SqlResult::Rows, got {other:?}"),
    }
}

fn assert_child_gone(svc: &mut DoogatService, child_id: &str, via: &str) {
    let Err(err) = svc.read_doogat(child_id) else {
        panic!("H3: CASCADE child {child_id} still readable from git after {via} deleted its parent");
    };
    assert!(
        matches!(err, DoogatError::NotFound(_)),
        "child read must fail with NotFound after {via}, got: {err:?}"
    );
    let rows = child_rows(svc, child_id);
    assert!(
        rows.is_empty(),
        "H3: CASCADE child {child_id} still has a membership row after {via}: {rows:?}"
    );
}

#[test]
#[ignore = "fast-track FT-3: hazard H3 confirmed 2026-09-06 (SQL DELETE runs no ON DELETE CASCADE walk); un-ignore with the fix, see dev/local/plans/fast-track-2026-09-06.md"]
fn sql_delete_of_cascade_parent_removes_child_or_refuses() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    let (parent_id, child_id) = seed_cascade_pair(&mut svc);
    assert_eq!(
        child_rows(&mut svc, &child_id).len(),
        1,
        "fixture: child membership row must exist before the delete"
    );

    match svc.execute_sql(&format!("DELETE FROM link WHERE id = '{parent_id}'")) {
        Ok(result) => {
            assert!(
                matches!(result, SqlResult::Affected(1)),
                "SQL DELETE of an existing parent must report Affected(1), got: {result:?}"
            );
            svc.read_doogat(&parent_id)
                .expect_err("parent must be gone after a successful SQL DELETE");
            assert_child_gone(&mut svc, &child_id, "SQL DELETE FROM link");
        }
        Err(err) => {
            // A RESTRICT-like refusal is acceptable only if it is structured,
            // names the child table, and leaves both rows untouched.
            assert!(
                matches!(err, DoogatError::Structured { .. }),
                "SQL DELETE refusal must be a structured error, got: {err:?}"
            );
            let msg = format!("{err}");
            assert!(
                msg.contains("membership"),
                "SQL DELETE refusal must name the child table, got: {msg}"
            );
            svc.read_doogat(&parent_id)
                .expect("refused SQL DELETE must keep the parent in git");
            svc.read_doogat(&child_id)
                .expect("refused SQL DELETE must keep the child in git");
        }
    }
}

#[test]
fn service_delete_of_cascade_parent_removes_child() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    let (parent_id, child_id) = seed_cascade_pair(&mut svc);

    svc.delete_doogat(&parent_id, "delete parent")
        .expect("service CASCADE delete must succeed");

    svc.read_doogat(&parent_id)
        .expect_err("parent must be gone after delete_doogat");
    assert_child_gone(&mut svc, &child_id, "DoogatService::delete_doogat");
}
