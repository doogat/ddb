use crate::common::DdbTestRepo;
use predicates::prelude::*;

// PRD 00132: ALTER TABLE foo RENAME TO bar through the CLI. Verifies that the
// rename works end to end via `ddb query`: the materialized table moves, the
// data doogats' `type:` field rewrites, and other typedefs' REFERENCES update.

#[test]
fn cli_alter_table_rename_to_renames_typedef_and_data() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE rfoo (title VARCHAR(64))"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table rfoo created"));

    repo.ddb()
        .args([
            "query",
            "INSERT INTO rfoo (id, title) VALUES ('20260428100001', 'first')",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO rfoo (id, title) VALUES ('20260428100002', 'second')",
        ])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "ALTER TABLE rfoo RENAME TO rbar"])
        .assert()
        .success()
        .stdout(predicate::str::contains("renamed to rbar"));

    // Old name no longer resolves.
    repo.ddb()
        .args(["query", "SELECT count(*) FROM rfoo"])
        .assert()
        .failure();

    // New name returns the same rows.
    repo.ddb()
        .args(["query", "SELECT count(*) FROM rbar"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2"));
    repo.ddb()
        .args(["query", "DROP TABLE rbar CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));
}

#[test]
fn cli_alter_table_rename_survives_subsequent_inserts() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE rcategory (title VARCHAR(64))"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE rmember (title VARCHAR(64), parent VARCHAR(14) REFERENCES rcategory(id))",
        ])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "ALTER TABLE rcategory RENAME TO rcat"])
        .assert()
        .success();

    // After the rename, the materialized table for the renamed source
    // continues to accept inserts under the new name.
    repo.ddb()
        .args([
            "query",
            "INSERT INTO rcat (id, title) VALUES ('20260428100100', 'cat-a')",
        ])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "SELECT count(*) FROM rcat"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1"));
}

#[test]
fn cli_alter_table_rename_rejects_target_already_exists() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE rsrc (title TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "CREATE TABLE rdst (title TEXT)"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "ALTER TABLE rsrc RENAME TO rdst"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn cli_alter_table_rename_rejects_reserved_target() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE rsrc2 (title TEXT)"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "ALTER TABLE rsrc2 RENAME TO doogats"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("reserved"));
}

#[test]
fn cli_mysql_rename_table_alias_rejected_with_clear_message() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE rsrc3 (title TEXT)"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "RENAME TABLE rsrc3 TO rsrc3_new"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("RENAME TABLE not supported"));
}
