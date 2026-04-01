use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn insert_without_date_derives_from_id() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE events (name TEXT, priority INTEGER)",
        ])
        .assert()
        .success();

    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO events (name, priority) VALUES ('Launch', 1)",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Derive expected date from 14-digit ID: YYYYMMDDHHmmss → YYYY-MM-DD
    let expected_date = format!("{}-{}-{}", &id[0..4], &id[4..6], &id[6..8]);

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("date: {expected_date}")));
}

#[test]
fn insert_with_explicit_date_preserves_value() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE meetings (name TEXT)",
        ])
        .assert()
        .success();

    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO meetings (name, date) VALUES ('Standup', '2025-01-15')",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("date: 2025-01-15"));
}
