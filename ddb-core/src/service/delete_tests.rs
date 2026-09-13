use super::DoogatService;
use crate::error::{codes, DoogatError};
use crate::sql_engine::SqlResult;

fn fresh() -> (tempfile::TempDir, DoogatService) {
    let tmp = tempfile::tempdir().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();
    svc.reindex().unwrap();
    (tmp, svc)
}

fn insert(svc: &mut DoogatService, sql: &str) -> String {
    match svc.execute_sql(sql).unwrap() {
        SqlResult::Ok(id) => id,
        result => panic!("expected an inserted id: {result:?}"),
    }
}

fn pair(svc: &mut DoogatService) -> (String, String) {
    svc.execute_sql("CREATE TABLE parent (name TEXT)").unwrap();
    svc.execute_sql("CREATE TABLE child (name TEXT, parent VARCHAR(255) NOT NULL REFERENCES parent(id) ON DELETE CASCADE)").unwrap();
    let parent = insert(svc, "INSERT INTO parent (name) VALUES ('parent')");
    let child = insert(
        svc,
        &format!("INSERT INTO child (name, parent) VALUES ('child', '{parent}')"),
    );
    (parent, child)
}

fn count(svc: &DoogatService, table: &str) -> i64 {
    svc.index
        .conn
        .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn delete_unregistered_type_without_a_materialized_table_still_succeeds() {
    let (_tmp, svc) = fresh();
    let id = "20260101000000";
    let path = format!("ddb/{id}.md");
    let content = format!("---\nid: {id}\ntitle: Legacy\ntype: legacy\n---\n");
    svc.repo
        .commit_file(&path, &content, "import legacy document")
        .unwrap();
    svc.index
        .index_doogat(&crate::parser::parse(&content, &path).unwrap())
        .unwrap();
    svc.index
        .store_head(&svc.repo.head_oid().unwrap().0)
        .unwrap();
    svc.delete_doogat(id, "delete legacy document").unwrap();
    assert!(svc.repo.read_file(&path).is_err());
    assert!(svc.index.resolve_path(id).is_err());
}

#[test]
fn cascade_delete_deduplicates_two_columns_on_both_paths() {
    for sql_path in [false, true] {
        let (_tmp, mut svc) = fresh();
        let (parent, child) = pair(&mut svc);
        svc.execute_sql("ALTER TABLE child ADD COLUMN other VARCHAR(255) REFERENCES parent(id) ON DELETE CASCADE").unwrap();
        svc.execute_sql(&format!(
            "UPDATE child SET other = '{parent}' WHERE id = '{child}'"
        ))
        .unwrap();
        let child_path = svc.index.resolve_path(&child).unwrap();
        let before = svc.repo.head_oid().unwrap();
        if sql_path {
            assert!(matches!(
                svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
                    .unwrap(),
                SqlResult::Affected(1)
            ));
        } else {
            svc.delete_doogat(&parent, "delete parent").unwrap();
        }
        assert_eq!(count(&svc, "parent"), 0);
        assert_eq!(count(&svc, "child"), 0);
        assert_eq!(count(&svc, "child_parent"), 0);
        assert_eq!(count(&svc, "child_other"), 0);
        assert!(svc.repo.read_file(&child_path).is_err());
        let head = svc.repo.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_id(0).unwrap().to_string(), before.0);
    }
}

#[test]
fn cascade_delete_bulk_deduplicates_shared_descendants() {
    let (_tmp, mut svc) = fresh();
    let (first, child) = pair(&mut svc);
    let second = insert(&mut svc, "INSERT INTO parent (name) VALUES ('second')");
    svc.execute_sql(
        "ALTER TABLE child ADD COLUMN other VARCHAR(255) REFERENCES parent(id) ON DELETE CASCADE",
    )
    .unwrap();
    svc.execute_sql(&format!(
        "UPDATE child SET other = '{second}' WHERE id = '{child}'"
    ))
    .unwrap();
    let first_path = svc.index.resolve_path(&first).unwrap();
    let second_path = svc.index.resolve_path(&second).unwrap();
    let child_path = svc.index.resolve_path(&child).unwrap();
    assert!(matches!(
        svc.execute_sql("DELETE FROM parent").unwrap(),
        SqlResult::Affected(2)
    ));
    for path in [first_path, second_path, child_path] {
        assert!(svc.repo.read_file(&path).is_err());
    }
    assert_eq!(count(&svc, "parent"), 0);
    assert_eq!(count(&svc, "child"), 0);
}

#[test]
fn cascade_delete_restrict_on_a_descendant_keeps_every_row() {
    for sql_path in [false, true] {
        let (_tmp, mut svc) = fresh();
        let (parent, child) = pair(&mut svc);
        svc.execute_sql(
            "CREATE TABLE blocker (name TEXT, child VARCHAR(255) NOT NULL REFERENCES child(id))",
        )
        .unwrap();
        insert(
            &mut svc,
            &format!("INSERT INTO blocker (name, child) VALUES ('blocker', '{child}')"),
        );
        let before = svc.repo.head_oid().unwrap();
        let err = if sql_path {
            svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
                .unwrap_err()
        } else {
            svc.delete_doogat(&parent, "delete parent").unwrap_err()
        };
        assert!(matches!(
            err,
            DoogatError::Structured {
                code: codes::REFERENCES_VIOLATION,
                ..
            }
        ));
        assert_eq!(svc.repo.head_oid().unwrap(), before);
        for table in [
            "parent",
            "child",
            "blocker",
            "child_parent",
            "blocker_child",
        ] {
            assert_eq!(count(&svc, table), 1, "{table} changed during refusal");
        }
    }
}

#[test]
fn cascade_delete_failure_rolls_back_prior_index_deletions() {
    for sql_path in [false, true] {
        let (_tmp, mut svc) = fresh();
        let (parent, _child) = pair(&mut svc);
        svc.reindex().unwrap();
        svc.index.conn.execute_batch("CREATE TRIGGER reject_child_delete BEFORE DELETE ON child BEGIN SELECT RAISE(ABORT, 'test child failure'); END").unwrap();
        let before = svc.repo.head_oid().unwrap();
        if sql_path {
            svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
                .unwrap_err();
        } else {
            svc.delete_doogat(&parent, "delete parent").unwrap_err();
        }
        assert_eq!(svc.repo.head_oid().unwrap(), before);
        assert_eq!(count(&svc, "parent"), 1);
        assert_eq!(count(&svc, "child"), 1);
        assert_eq!(count(&svc, "child_parent"), 1);
        assert_eq!(count(&svc, "doogats"), 4);
    }
}

#[test]
fn cascade_delete_sql_rollback_restores_children_and_junctions() {
    let (_tmp, mut svc) = fresh();
    let (parent, child) = pair(&mut svc);
    let child_path = svc.index.resolve_path(&child).unwrap();
    let before = svc.repo.head_oid().unwrap();
    svc.execute_sql("BEGIN").unwrap();
    svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
        .unwrap();
    assert_eq!(count(&svc, "child"), 0);
    assert_eq!(count(&svc, "child_parent"), 0);
    assert_eq!(svc.repo.head_oid().unwrap(), before);
    assert!(svc.repo.read_file(&child_path).is_ok());
    svc.execute_sql("ROLLBACK").unwrap();
    assert_eq!(count(&svc, "parent"), 1);
    assert_eq!(count(&svc, "child"), 1);
    assert_eq!(count(&svc, "child_parent"), 1);
    assert_eq!(svc.repo.head_oid().unwrap(), before);
}

#[test]
fn cascade_delete_failure_inside_transaction_keeps_prior_write() {
    let (_tmp, mut svc) = fresh();
    let (parent, _child) = pair(&mut svc);
    svc.reindex().unwrap();
    svc.index.conn.execute_batch("CREATE TRIGGER reject_child_delete BEFORE DELETE ON child BEGIN SELECT RAISE(ABORT, 'test child failure'); END").unwrap();
    svc.execute_sql("BEGIN").unwrap();
    let kept = insert(&mut svc, "INSERT INTO parent (name) VALUES ('kept')");
    svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
        .unwrap_err();
    assert_eq!(count(&svc, "parent"), 2);
    assert_eq!(count(&svc, "child"), 1);
    assert!(svc.txn.as_ref().unwrap().deletes.is_empty());
    svc.execute_sql("COMMIT").unwrap();
    assert!(svc.read_doogat(&kept).is_ok());
    assert!(svc.read_doogat(&parent).is_ok());
}

#[test]
fn cascade_delete_chain_reaches_grandchildren_in_one_commit() {
    for sql_path in [false, true] {
        let (_tmp, mut svc) = fresh();
        let (parent, child) = pair(&mut svc);
        svc.execute_sql("CREATE TABLE grandchild (name TEXT, child VARCHAR(255) REFERENCES child(id) ON DELETE CASCADE)").unwrap();
        let grandchild = insert(
            &mut svc,
            &format!("INSERT INTO grandchild (name, child) VALUES ('grandchild', '{child}')"),
        );
        let path = svc.index.resolve_path(&grandchild).unwrap();
        let before = svc.repo.head_oid().unwrap();
        if sql_path {
            svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
                .unwrap();
        } else {
            svc.delete_doogat(&parent, "delete parent").unwrap();
        }
        for table in [
            "parent",
            "child",
            "grandchild",
            "child_parent",
            "grandchild_child",
        ] {
            assert_eq!(count(&svc, table), 0);
        }
        assert!(svc.repo.read_file(&path).is_err());
        let head = svc.repo.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_id(0).unwrap().to_string(), before.0);
    }
}

#[test]
fn cascade_delete_reads_buffered_typedef_and_child() {
    let (_tmp, mut svc) = fresh();
    svc.execute_sql("CREATE TABLE parent (name TEXT)").unwrap();
    let parent = insert(&mut svc, "INSERT INTO parent (name) VALUES ('parent')");
    let before = svc.repo.head_oid().unwrap();
    svc.execute_sql("BEGIN").unwrap();
    svc.execute_sql("CREATE TABLE child (name TEXT, parent VARCHAR(255) REFERENCES parent(id) ON DELETE CASCADE)").unwrap();
    let child = insert(
        &mut svc,
        &format!("INSERT INTO child (name, parent) VALUES ('child', '{parent}')"),
    );
    svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
        .unwrap();
    assert_eq!(count(&svc, "child"), 0);
    assert_eq!(svc.repo.head_oid().unwrap(), before);
    svc.execute_sql("COMMIT").unwrap();
    assert!(svc.read_doogat(&parent).is_err());
    assert!(svc.read_doogat(&child).is_err());
    assert_eq!(count(&svc, "child_parent"), 0);
}

#[test]
fn cascade_delete_composes_surviving_references_to_multiple_targets() {
    for sql_path in [false, true] {
        let (_tmp, mut svc) = fresh();
        let (parent, child) = pair(&mut svc);
        svc.execute_sql("CREATE TABLE survivor (name TEXT, parent VARCHAR(255) REFERENCES parent(id), child VARCHAR(255) REFERENCES child(id))").unwrap();
        let survivor = insert(&mut svc, &format!("INSERT INTO survivor (name, parent, child) VALUES ('survivor', '{parent}', '{child}')"));
        let path = svc.index.resolve_path(&survivor).unwrap();
        if sql_path {
            svc.execute_sql(&format!("DELETE FROM parent WHERE id = '{parent}'"))
                .unwrap();
        } else {
            svc.delete_doogat(&parent, "delete parent").unwrap();
        }
        let content = svc.repo.read_file(&path).unwrap();
        assert!(
            !content.contains(&parent),
            "parent reference survived: {content}"
        );
        assert!(
            !content.contains(&child),
            "child reference survived: {content}"
        );
        assert_eq!(count(&svc, "survivor"), 1);
        assert_eq!(count(&svc, "survivor_parent"), 0);
        assert_eq!(count(&svc, "survivor_child"), 0);
    }
}
