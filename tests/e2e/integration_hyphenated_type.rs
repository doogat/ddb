//! Ports tests/integration.sh §36 (lines 3366-3372): hyphenated type name
//! via double-quoted SQL identifiers.

use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn integration_36_hyphenated_type_create_table() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE \"my-type\" (label TEXT)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table my-type created"));

    let out = repo
        .ddb()
        .args(["query", "INSERT INTO \"my-type\" (label) VALUES ('test')"])
        .output()
        .unwrap();
    let my_id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["query", "SELECT label FROM \"my-type\""])
        .assert()
        .success()
        .stdout(predicate::str::contains("test"));

    repo.ddb()
        .args([
            "query",
            &format!("DELETE FROM \"my-type\" WHERE id = '{my_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));
}
