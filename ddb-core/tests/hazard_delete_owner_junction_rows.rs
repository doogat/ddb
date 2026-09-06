//! Hazard H2: service-path delete of a reference OWNER leaves its auto-junction rows.
//!
//! `DoogatService::delete_doogat` (ddb-core/src/service/delete.rs:107) calls
//! `Index::cascade_junction_cleanup` (ddb-core/src/indexer/mod.rs:480-500), which
//! sweeps only the REVERSE direction: junction rows where the deleted id is the
//! referenced target (`<other>_<col>.<col>_id = id`). The SQL `DELETE` path
//! (ddb-core/src/sql_engine/dml.rs:1266-1332) additionally runs the OWNER
//! direction (`<type>_<col>.<type>_id = id`, dml.rs:1321-1327). Deleting the
//! owner of a REFERENCES value through the service (CLI `ddb delete`, GraphQL,
//! REST, FFI) may therefore strand its row in `bookmark_category` keyed by
//! `bookmark_id`. `tests/e2e/cascade_delete.rs` pins only the SQL-DELETE owner
//! case (:90-181) and the service target-side case (:263-343); the service
//! owner case is unpinned.
//!
//! A failure here means the hazard is real: the service delete path leaves
//! owner-side junction rows behind. A pass makes this file a regression pin.

use ddb_core::indexer::{junction_parent_id_column, junction_ref_id_column, junction_table_name};
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;

fn insert_id(svc: &mut DoogatService, sql: &str) -> String {
    match svc.execute_sql(sql).expect("fixture INSERT must succeed") {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok(id) from INSERT, got {other:?}"),
    }
}

fn count_rows(svc: &mut DoogatService, sql: &str) -> String {
    match svc.execute_sql(sql).expect("COUNT query must succeed") {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(
                rows.len(),
                1,
                "COUNT(*) must yield exactly one row, got {rows:?}"
            );
            rows[0][0].clone()
        }
        other => panic!("COUNT(*) must produce Rows, got: {other:?}"),
    }
}

#[test]
#[ignore = "fast-track FT-2: hazard H2 confirmed 2026-09-06 (service delete leaves owner-side junction rows); un-ignore with the fix, see dev/local/plans/fast-track-2026-09-06.md"]
fn service_delete_of_reference_owner_removes_its_junction_rows() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();

    svc.execute_sql("CREATE TABLE category (label VARCHAR(100))")
        .expect("CREATE TABLE category must succeed");
    svc.execute_sql("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .expect("CREATE TABLE bookmark must succeed");

    // Target T: the referenced category.
    let cat_id = insert_id(&mut svc, "INSERT INTO category (label) VALUES ('alpha')");
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Owner O: setting the REFERENCES column materializes the junction row.
    let owner_insert = format!(
        "INSERT INTO bookmark (url, category) VALUES ('https://example.com', '{cat_id}')"
    );
    let bm_id = insert_id(&mut svc, &owner_insert);

    let jt = junction_table_name("bookmark", "category");
    let parent_col = junction_parent_id_column("bookmark");
    let ref_col = junction_ref_id_column("category");
    assert_eq!(jt, "bookmark_category");
    assert_eq!(parent_col, "bookmark_id");
    assert_eq!(ref_col, "category_id");

    let owner_rows_sql = format!("SELECT COUNT(*) FROM {jt} WHERE {parent_col} = '{bm_id}'");
    assert_eq!(
        count_rows(&mut svc, &owner_rows_sql),
        "1",
        "fixture: owner {bm_id} must hold exactly one row in {jt} before the delete"
    );

    // Delete the OWNER through the service path (not the SQL DELETE path).
    svc.delete_doogat(&bm_id, "delete junction owner via service")
        .expect("delete_doogat on the junction owner must succeed");

    // Sanity: the typed row is gone, so any surviving junction row is stranded.
    let typed_rows_sql = format!("SELECT COUNT(*) FROM bookmark WHERE id = '{bm_id}'");
    assert_eq!(
        count_rows(&mut svc, &typed_rows_sql),
        "0",
        "delete_doogat must remove the bookmark typed row for {bm_id}"
    );

    assert_eq!(
        count_rows(&mut svc, &owner_rows_sql),
        "0",
        "HAZARD H2 fired: delete_doogat left owner-side row(s) in {jt} where \
         {parent_col} = {bm_id}. The service path runs the reverse-only \
         Index::cascade_junction_cleanup (indexer/mod.rs:480) while SQL DELETE \
         also cleans the owner direction (sql_engine/dml.rs:1321)"
    );

    // Stronger: the only junction row was the deleted owner's, so the table is empty.
    let all_rows_sql = format!("SELECT COUNT(*) FROM {jt}");
    assert_eq!(
        count_rows(&mut svc, &all_rows_sql),
        "0",
        "HAZARD H2 fired: {jt} must be empty after its only owner {bm_id} was \
         deleted through the service path (target {cat_id} still exists)"
    );
}
