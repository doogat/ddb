//! PRD 00139 §8 / T21: e2e coverage for `ALTER TABLE x SET SINGLETON` and
//! `ALTER TABLE x DROP SINGLETON` through the actual `ddb` binary.

use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn set_singleton_succeeds_on_empty_typedef() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE empty_cfg (theme TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "ALTER TABLE empty_cfg SET SINGLETON"])
        .assert()
        .success()
        .stdout(predicate::str::contains("singleton set on empty_cfg"));
}

#[test]
fn set_singleton_succeeds_on_one_row_typedef() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE one_row_cfg (theme TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO one_row_cfg (title, theme) VALUES ('only', 'dark')",
        ])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "ALTER TABLE one_row_cfg SET SINGLETON"])
        .assert()
        .success();
}

#[test]
fn set_singleton_rejects_multi_row_typedef_with_count() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE multi_cfg (theme TEXT)"])
        .assert()
        .success();
    for theme in &["dark", "light"] {
        repo.ddb()
            .args([
                "query",
                &format!("INSERT INTO multi_cfg (title, theme) VALUES ('t', '{theme}')"),
            ])
            .assert()
            .success();
    }
    repo.ddb()
        .args(["query", "ALTER TABLE multi_cfg SET SINGLETON"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("typedef holds 2 rows"))
        .stderr(predicate::str::contains("SET SINGLETON"));
}

#[test]
fn drop_singleton_succeeds_and_is_idempotent() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE drop_cfg (theme TEXT) SINGLETON"])
        .assert()
        .success();
    // First DROP succeeds.
    repo.ddb()
        .args(["query", "ALTER TABLE drop_cfg DROP SINGLETON"])
        .assert()
        .success()
        .stdout(predicate::str::contains("singleton dropped on drop_cfg"));
    // Second DROP is idempotent (succeeds with already-cleared message).
    repo.ddb()
        .args(["query", "ALTER TABLE drop_cfg DROP SINGLETON"])
        .assert()
        .success()
        .stdout(predicate::str::contains("singleton already cleared"));
}

#[test]
fn set_drop_set_round_trip_keeps_data() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE rt_cfg (theme TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO rt_cfg (title, theme) VALUES ('only', 'dark')",
        ])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "ALTER TABLE rt_cfg SET SINGLETON"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "ALTER TABLE rt_cfg DROP SINGLETON"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "ALTER TABLE rt_cfg SET SINGLETON"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "SELECT theme FROM rt_cfg"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dark"));
}

#[test]
fn after_set_singleton_second_insert_rejects_with_constraint() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE post_alter_cfg (theme TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO post_alter_cfg (title, theme) VALUES ('one', 'dark')",
        ])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "ALTER TABLE post_alter_cfg SET SINGLETON"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO post_alter_cfg (title, theme) VALUES ('two', 'light')",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("SINGLETON constraint"));
}
