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

    // Insert using COALESCE with subquery - first row gets 0
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
        .args([
            "query",
            "CREATE TABLE items (name TEXT, label TEXT)",
        ])
        .assert()
        .success();

    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO items (name) VALUES ('thing')",
        ])
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
            &format!(
                "UPDATE items SET label = IFNULL(NULL, 'default') WHERE id = '{id}'"
            ),
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
        .args([
            "query",
            "CREATE TABLE ordered (name TEXT, pos INTEGER)",
        ])
        .assert()
        .success();

    // Seed a row with pos = 5
    repo.ddb()
        .args([
            "query",
            "INSERT INTO ordered (name, pos) VALUES ('a', 5)",
        ])
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
