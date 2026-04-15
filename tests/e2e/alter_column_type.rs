use crate::common::DdbTestRepo;
use predicates::prelude::*;

// PRD 00128: widen VARCHAR(255) to TEXT, then insert a long value that would
// have been rejected pre-ALTER. Verify persistence across a fresh process.

#[test]
fn widens_varchar_to_text_and_accepts_longer_values() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE link (url VARCHAR(255))"])
        .assert()
        .success();

    let url_255 = "a".repeat(255);
    repo.ddb()
        .args([
            "query",
            &format!("INSERT INTO link (title, url) VALUES ('boundary', '{url_255}')"),
        ])
        .assert()
        .success();

    let url_2000 = "b".repeat(2000);
    repo.ddb()
        .args([
            "query",
            &format!("INSERT INTO link (title, url) VALUES ('toolong', '{url_2000}')"),
        ])
        .assert()
        .failure();

    repo.ddb()
        .args(["query", "ALTER TABLE link ALTER COLUMN url TYPE TEXT"])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            &format!("INSERT INTO link (title, url) VALUES ('now-ok', '{url_2000}')"),
        ])
        .assert()
        .success();

    let url_another = "c".repeat(1500);
    repo.ddb()
        .args([
            "query",
            &format!("INSERT INTO link (title, url) VALUES ('persisted', '{url_another}')"),
        ])
        .assert()
        .success();
}

#[test]
fn narrowing_rejects_with_row_count_message() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE narrow (body VARCHAR(100))"])
        .assert()
        .success();

    let long = "x".repeat(80);
    repo.ddb()
        .args([
            "query",
            &format!("INSERT INTO narrow (title, body) VALUES ('t', '{long}')"),
        ])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "ALTER TABLE narrow ALTER COLUMN body TYPE VARCHAR(20)",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot narrow"))
        .stderr(predicate::str::contains("1 existing rows"));
}
