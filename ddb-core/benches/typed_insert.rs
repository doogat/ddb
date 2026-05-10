//! PRD 00134 §T6: bench typed-INSERT-with-REFERENCES at 1000 rows.
//!
//! The pre-existing `crud.rs` bench is git-only — it never touches
//! `SqlEngine::insert_materialized_row`, `populate_junction_tables`, or any
//! typed-write codepath. PRD 00134 added auto-junction population on
//! INSERT/UPDATE inside a SAVEPOINT; this bench exercises that hot path so
//! a regression in the savepoint setup or junction insert shows up.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;
use tempfile::TempDir;

const REF_COUNT: usize = 100;
const ROW_COUNT: usize = 1000;

/// Initialize a service with a typedef pair (`category` + `link` REFERENCES
/// `category`) and `REF_COUNT` pre-populated category rows. Returns the
/// service and the list of category ids the link bench iterations will
/// reference.
fn fresh_service_with_refs(
    dir: &std::path::Path,
) -> (DoogatService<ddb_core::git_ops::GitRepo>, Vec<String>) {
    let mut svc = DoogatService::init(dir).unwrap();
    svc.execute_sql("CREATE TABLE category (label VARCHAR(64))")
        .unwrap();
    svc.execute_sql("CREATE TABLE link (target VARCHAR(64) REFERENCES category)")
        .unwrap();

    let mut cat_ids = Vec::with_capacity(REF_COUNT);
    for i in 0..REF_COUNT {
        let id = match svc
            .execute_sql(&format!("INSERT INTO category (label) VALUES ('cat-{i}')"))
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            other => panic!("expected Ok, got {other:?}"),
        };
        cat_ids.push(id);
    }
    (svc, cat_ids)
}

/// Bench `ROW_COUNT` typed INSERTs each carrying a REFERENCES value. Each
/// statement runs the full `SAVEPOINT insert_row → insert_materialized_row →
/// populate_junction_tables` pipeline, including the per-row junction
/// INSERT. One bench sample = `ROW_COUNT` rows, so the headline number is
/// roughly amortized per-row.
fn bench_typed_insert_with_references(c: &mut Criterion) {
    c.bench_function("crud/typed_insert_with_references_1k", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().unwrap();
                let (svc, cat_ids) = fresh_service_with_refs(dir.path());
                (dir, svc, cat_ids)
            },
            |(_dir, mut svc, cat_ids)| {
                for i in 0..ROW_COUNT {
                    let cat = &cat_ids[i % REF_COUNT];
                    svc.execute_sql(&format!(
                        "INSERT INTO link (title, target) VALUES ('link-{i}', '{cat}')"
                    ))
                    .unwrap();
                }
            },
            BatchSize::PerIteration,
        );
    });
}

criterion_group!(benches, bench_typed_insert_with_references);
criterion_main!(benches);
