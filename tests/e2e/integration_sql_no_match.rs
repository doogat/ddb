//! Ports tests/integration.sh §30 no-match half (lines 3240-3256) and §30.F1
//! (lines 3258-3270): WHERE-id no-match semantics for UPDATE/DELETE, and CLI
//! propagation of the composite-UNIQUE duplicate rejection error.

use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn integration_30_where_id_no_match_semantics() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE smokenomatch (name TEXT, score INTEGER)",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table smokenomatch created"));

    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO smokenomatch (name, score) VALUES ('alpha', 1)",
        ])
        .output()
        .unwrap();
    let nomatch_id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // B1: UPDATE with a nonexistent id is a normal success, 0 rows affected.
    repo.ddb()
        .args([
            "query",
            "UPDATE smokenomatch SET score = 1 WHERE id = 'nonexistent_id_00000000000000'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 row(s) affected"));

    // B2: DELETE with a nonexistent id is a normal success, 0 rows affected.
    repo.ddb()
        .args([
            "query",
            "DELETE FROM smokenomatch WHERE id = 'nonexistent_id_00000000000000'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 row(s) affected"));

    // B3: IN-list mixing a missing id and the valid id still affects 1 row.
    repo.ddb()
        .args([
            "query",
            &format!("UPDATE smokenomatch SET score = 7 WHERE id IN ('nope', '{nomatch_id}')"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));

    // B4: valid id + non-matching second predicate affects 0 rows.
    repo.ddb()
        .args([
            "query",
            &format!(
                "UPDATE smokenomatch SET score = 99 WHERE id = '{nomatch_id}' AND name = 'wrongname'"
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 row(s) affected"));

    // B5: valid id on the fast path affects 1 row, and the update applied.
    repo.ddb()
        .args([
            "query",
            &format!("UPDATE smokenomatch SET score = 42 WHERE id = '{nomatch_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));

    repo.ddb()
        .args([
            "query",
            &format!("SELECT score FROM smokenomatch WHERE id = '{nomatch_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("42"));
}

#[test]
fn integration_30_f1_composite_unique_cli_rejection() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE f1mship (title VARCHAR(255), link_id VARCHAR(255), category VARCHAR(255), UNIQUE(link_id, category))",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table f1mship created"));

    repo.ddb()
        .args([
            "query",
            "INSERT INTO f1mship (title, link_id, category) VALUES ('a', 'link1', 'cat1')",
        ])
        .assert()
        .success();

    // Same (link_id, category) pair again — must be rejected.
    let out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO f1mship (title, link_id, category) VALUES ('b', 'link1', 'cat1')",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("UNIQUE"));
    assert!(
        combined.contains("f1mship") || combined.contains("link_id") || combined.contains("category"),
        "expected table/column context in error, got: {combined}"
    );
}
