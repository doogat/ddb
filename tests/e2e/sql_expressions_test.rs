use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn insert_with_coalesce() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE tasks (name TEXT, sort_order INTEGER)",
        ])
        .assert()
        .success();

    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO tasks (name, sort_order) VALUES ('first', COALESCE((SELECT MAX(sort_order) FROM tasks), 0))",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "insert with COALESCE failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    repo.ddb()
        .args(["query", "SELECT name, sort_order FROM tasks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("first | 0"));
}

#[test]
fn update_with_ifnull() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE items (name TEXT, label TEXT)"])
        .assert()
        .success();

    let out = repo
        .ddb()
        .args(["query", "INSERT INTO items (name) VALUES ('thing')"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "insert failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // UPDATE with IFNULL - uses literal NULL so result is 'default'
    repo.ddb()
        .args([
            "query",
            &format!("UPDATE items SET label = IFNULL(NULL, 'default') WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));

    repo.ddb()
        .args([
            "query",
            &format!("SELECT name, label FROM items WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("thing | default"));
}

#[test]
fn insert_with_arithmetic() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE ordered (name TEXT, pos INTEGER)"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "INSERT INTO ordered (name, pos) VALUES ('a', 5)"])
        .assert()
        .success();

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Insert with subquery arithmetic: MAX(pos) + 1 = 6
    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO ordered (name, pos) VALUES ('b', (SELECT MAX(pos) FROM ordered) + 1)",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "insert with arithmetic failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    repo.ddb()
        .args(["query", "SELECT name, pos FROM ordered ORDER BY pos"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a | 5"))
        .stdout(predicate::str::contains("b | 6"));
}

#[test]
fn nullif_in_insert() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE nulltest (label TEXT)"])
        .assert()
        .success();

    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO nulltest (label) VALUES (NULLIF('', ''))",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "insert with NULLIF failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // NULLIF('','') returns NULL — ddb displays as "NULL"
    repo.ddb()
        .args([
            "query",
            &format!("SELECT label FROM nulltest WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("NULL"));
}

#[test]
fn update_with_ifnull_column_ref() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE colref (label TEXT)"])
        .assert()
        .success();

    // Insert without specifying label — label will be NULL/empty in materialized table
    let out = repo
        .ddb()
        .args(["query", "INSERT INTO colref (label) VALUES ('')"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "insert failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // UPDATE with IFNULL referencing the column — empty string is not NULL in SQLite,
    // so IFNULL(label, 'default') returns '' (the existing value). Use NULLIF to
    // convert empty to NULL first: IFNULL(NULLIF(label, ''), 'default') -> 'default'
    repo.ddb()
        .args([
            "query",
            &format!(
                "UPDATE colref SET label = IFNULL(NULLIF(label, ''), 'default') WHERE id = '{id}'"
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));

    repo.ddb()
        .args([
            "query",
            &format!("SELECT label FROM colref WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("default"));
}

#[test]
fn insert_with_nested_expression() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE nested (val INTEGER)"])
        .assert()
        .success();

    // ABS(-1) = 1, LENGTH('hi') = 2, so COALESCE(1 + 2, 0) = 3
    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO nested (val) VALUES (COALESCE(ABS(-1) + LENGTH('hi'), 0))",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "insert with nested expression failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args([
            "query",
            &format!("SELECT val FROM nested WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("3"));
}

#[test]
fn insert_rejects_malformed_expression() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE malformed (val INTEGER)"])
        .assert()
        .success();

    // COALESCE() with no args is malformed — should fail
    repo.ddb()
        .args(["query", "INSERT INTO malformed (val) VALUES (COALESCE())"])
        .assert()
        .failure();
}

#[test]
fn insert_rejects_unlisted_function() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE badexpr (name TEXT, val INTEGER)"])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "INSERT INTO badexpr (name, val) VALUES ('x', RANDOM())",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not allowed"));
}
