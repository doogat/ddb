// `ALTER TABLE <t> ADD UNIQUE (...)` / `DROP UNIQUE (...)` on existing types.
use super::setup;
use crate::git_ops::GitRepo;
use crate::indexer::Index;
use crate::sql_engine::{SqlEngine, SqlResult};
use std::collections::BTreeSet;
use std::path::Path;

/// Order-independent view of a typedef's `unique_together`. Fails if the
/// stored list holds an empty entry or the same constraint twice.
fn constraints(index: &Index, repo: &GitRepo, table: &str) -> BTreeSet<BTreeSet<String>> {
    let schemas = index.load_all_typedefs(repo);
    let raw = schemas
        .get(table)
        .unwrap_or_else(|| panic!("typedef {table} missing"))
        .unique_together
        .clone()
        .unwrap_or_default();
    let set: BTreeSet<BTreeSet<String>> = raw
        .iter()
        .map(|cols| cols.iter().cloned().collect())
        .collect();
    assert!(
        set.iter().all(|c| !c.is_empty()),
        "no empty constraint entries allowed: {raw:?}"
    );
    assert_eq!(set.len(), raw.len(), "duplicate constraint entries: {raw:?}");
    set
}

fn cset(groups: &[&[&str]]) -> BTreeSet<BTreeSet<String>> {
    groups
        .iter()
        .map(|g| g.iter().map(|c| c.to_string()).collect())
        .collect()
}

fn commit_count(dir: &Path) -> usize {
    let git = git2::Repository::open(dir).unwrap();
    let mut walk = git.revwalk().unwrap();
    walk.push_head().unwrap();
    walk.count()
}

fn ok_message(result: SqlResult) -> String {
    let SqlResult::Ok(msg) = result else {
        panic!("expected Ok variant");
    };
    msg
}

const ABC: &str = "(title TEXT, a VARCHAR(255), b VARCHAR(255), c VARCHAR(255)";

#[test]
fn add_unique_records_new_constraint_beside_existing_ones() {
    let (dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute(&format!("CREATE TABLE t1 {ABC}, UNIQUE(c))")).unwrap();
    engine.execute(&format!("CREATE TABLE t2 {ABC})")).unwrap();
    // Rows share `a` but differ on (a, b): not duplicates for the new pair.
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r1', 'A1', 'B1', 'C1')")
        .unwrap();
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r2', 'A1', 'B2', 'C2')")
        .unwrap();
    let before = commit_count(dir.path());
    let msg = ok_message(engine.execute("ALTER TABLE t1 ADD UNIQUE (a, b)").unwrap());
    assert!(!msg.contains("already"), "real add must not report no-op: {msg}");
    assert_eq!(constraints(&index, &repo, "t1"), cset(&[&["c"], &["a", "b"]]));
    assert!(
        constraints(&index, &repo, "t2").is_empty(),
        "ALTER on t1 must not touch t2"
    );
    assert_eq!(
        commit_count(dir.path()),
        before + 1,
        "a successful ADD UNIQUE lands as exactly one commit"
    );
}

#[test]
fn add_unique_enforces_constraint_on_insert_and_update() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute(&format!("CREATE TABLE t1 {ABC})")).unwrap();
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r1', 'A1', 'B1', 'C1')")
        .unwrap();
    engine.execute("ALTER TABLE t1 ADD UNIQUE (a, b)").unwrap();
    let dup = engine.execute("INSERT INTO t1 (title, a, b, c) VALUES ('r2', 'A1', 'B1', 'C2')");
    assert!(dup.is_err(), "duplicate INSERT must fail: {dup:?}");
    // Same `a`, different `b`: allowed.
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r3', 'A1', 'B3', 'C3')")
        .unwrap();
    let upd = engine.execute("UPDATE t1 SET b = 'B1' WHERE b = 'B3'");
    assert!(upd.is_err(), "UPDATE creating a duplicate must fail: {upd:?}");
}

#[test]
fn add_unique_rejects_duplicate_rows_and_leaves_typedef_unchanged() {
    let (dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute(&format!("CREATE TABLE t1 {ABC}, UNIQUE(c))")).unwrap();
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r1', 'A1', 'B1', 'C1')")
        .unwrap();
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r2', 'A1', 'B1', 'C2')")
        .unwrap();
    let before = commit_count(dir.path());
    let err = engine
        .execute("ALTER TABLE t1 ADD UNIQUE (a, b)")
        .expect_err("ADD UNIQUE over duplicate rows must fail");
    let msg = err.to_string();
    assert!(msg.contains("duplicate"), "msg = {msg}");
    assert_eq!(constraints(&index, &repo, "t1"), cset(&[&["c"]]));
    assert_eq!(commit_count(dir.path()), before, "rejected ADD must not commit");
    // No enforcement was installed: another (A1, B1) row still inserts.
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r3', 'A1', 'B1', 'C3')")
        .unwrap();
}

#[test]
fn add_unique_is_idempotent_regardless_of_column_order() {
    let (dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute(&format!("CREATE TABLE t1 {ABC}, UNIQUE(a, b))")).unwrap();
    let before = commit_count(dir.path());
    let msg = ok_message(engine.execute("ALTER TABLE t1 ADD UNIQUE (b, a)").unwrap());
    assert!(msg.contains("already present"), "msg = {msg}");
    assert_eq!(constraints(&index, &repo, "t1"), cset(&[&["a", "b"]]));
    assert_eq!(commit_count(dir.path()), before, "no-op re-apply must not commit");
    // A strict subset is a different constraint, not "already present".
    let msg = ok_message(engine.execute("ALTER TABLE t1 ADD UNIQUE (a)").unwrap());
    assert!(!msg.contains("already present"), "msg = {msg}");
    assert_eq!(
        constraints(&index, &repo, "t1"),
        cset(&[&["a", "b"], &["a"]])
    );
}

#[test]
fn add_unique_accepts_quoted_and_bare_names_with_optional_semicolon() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute(&format!("CREATE TABLE \"pair-link\" {ABC})"))
        .unwrap();
    engine.execute(&format!("CREATE TABLE plain {ABC})")).unwrap();
    engine
        .execute("ALTER TABLE \"pair-link\" ADD UNIQUE (a, c);")
        .unwrap();
    engine.execute("ALTER TABLE plain ADD UNIQUE (b)").unwrap();
    assert_eq!(constraints(&index, &repo, "pair-link"), cset(&[&["a", "c"]]));
    assert_eq!(constraints(&index, &repo, "plain"), cset(&[&["b"]]));
    engine
        .execute("ALTER TABLE \"pair-link\" DROP UNIQUE (c, a);")
        .unwrap();
    engine.execute("ALTER TABLE plain DROP UNIQUE (b)").unwrap();
    assert!(constraints(&index, &repo, "pair-link").is_empty());
    assert!(constraints(&index, &repo, "plain").is_empty());
}

#[test]
fn drop_unique_removes_only_named_constraint_and_its_enforcement() {
    let (dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute(&format!("CREATE TABLE t1 {ABC}, UNIQUE(a, b), UNIQUE(c))"))
        .unwrap();
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r1', 'A1', 'B1', 'C1')")
        .unwrap();
    let before = commit_count(dir.path());
    // Reversed column order versus how the constraint was stored.
    let msg = ok_message(engine.execute("ALTER TABLE t1 DROP UNIQUE (b, a)").unwrap());
    assert!(!msg.contains("already"), "real drop must not report no-op: {msg}");
    assert_eq!(constraints(&index, &repo, "t1"), cset(&[&["c"]]));
    assert_eq!(commit_count(dir.path()), before + 1);
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r2', 'A1', 'B1', 'C2')")
        .expect("(a, b) duplicate must succeed once the constraint is dropped");
    let dup_c = engine.execute("INSERT INTO t1 (title, a, b, c) VALUES ('r3', 'A9', 'B9', 'C1')");
    assert!(dup_c.is_err(), "UNIQUE(c) must still be enforced: {dup_c:?}");
}

#[test]
fn drop_unique_is_idempotent_when_constraint_absent() {
    let (dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute(&format!("CREATE TABLE t1 {ABC}, UNIQUE(a, b))")).unwrap();
    engine.execute(&format!("CREATE TABLE t2 {ABC})")).unwrap();
    engine
        .execute("INSERT INTO t1 (title, a, b, c) VALUES ('r1', 'A1', 'B1', 'C1')")
        .unwrap();
    let before = commit_count(dir.path());
    // `(a)` is a subset of the stored `(a, b)`, not the same constraint.
    let msg = ok_message(engine.execute("ALTER TABLE t1 DROP UNIQUE (a)").unwrap());
    assert!(msg.contains("already absent"), "msg = {msg}");
    let msg = ok_message(engine.execute("ALTER TABLE t2 DROP UNIQUE (a, b)").unwrap());
    assert!(msg.contains("already absent"), "msg = {msg}");
    assert_eq!(commit_count(dir.path()), before, "no-op drops must not commit");
    assert_eq!(constraints(&index, &repo, "t1"), cset(&[&["a", "b"]]));
    assert!(constraints(&index, &repo, "t2").is_empty());
    let dup = engine.execute("INSERT INTO t1 (title, a, b, c) VALUES ('r2', 'A1', 'B1', 'C2')");
    assert!(dup.is_err(), "(a, b) must still be enforced: {dup:?}");
}

#[test]
fn drop_last_unique_leaves_constraint_simply_absent() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute(&format!("CREATE TABLE t1 {ABC}, UNIQUE(a, b))")).unwrap();
    engine.execute("ALTER TABLE t1 DROP UNIQUE (a, b)").unwrap();
    // `constraints` asserts no empty entries remain in the stored list.
    assert!(constraints(&index, &repo, "t1").is_empty());
}

#[test]
fn add_drop_add_round_trip_matches_single_add() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute(&format!("CREATE TABLE rt {ABC})")).unwrap();
    engine.execute(&format!("CREATE TABLE once {ABC})")).unwrap();
    engine.execute("ALTER TABLE rt ADD UNIQUE (a, b)").unwrap();
    engine.execute("ALTER TABLE rt DROP UNIQUE (a, b)").unwrap();
    assert!(constraints(&index, &repo, "rt").is_empty());
    engine.execute("ALTER TABLE rt ADD UNIQUE (a, b)").unwrap();
    engine.execute("ALTER TABLE once ADD UNIQUE (a, b)").unwrap();
    assert_eq!(constraints(&index, &repo, "rt"), cset(&[&["a", "b"]]));
    assert_eq!(
        constraints(&index, &repo, "rt"),
        constraints(&index, &repo, "once")
    );
    for table in ["rt", "once"] {
        engine
            .execute(&format!("INSERT INTO {table} (title, a, b) VALUES ('r1', 'A1', 'B1')"))
            .unwrap();
        let dup =
            engine.execute(&format!("INSERT INTO {table} (title, a, b) VALUES ('r2', 'A1', 'B1')"));
        assert!(dup.is_err(), "{table}: duplicate must fail: {dup:?}");
    }
}
