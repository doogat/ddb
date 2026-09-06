//! NoSQL CLI commands (get, scan --tag, scan --type, backlinks)
//! exercised against a self-contained fixture.

use crate::common::{assert_doogat_id, DdbTestRepo};
use predicates::prelude::*;

#[test]
fn integration_32_nosql_cli_commands() {
    let repo = DdbTestRepo::init();

    let id2_out = repo
        .ddb()
        .args(["create", "--title", "Second note"])
        .output()
        .unwrap();
    let id2 = String::from_utf8_lossy(&id2_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Register a type so `scan --type` has something to find.
    repo.ddb()
        .args(["query", "CREATE TABLE foo (label TEXT)"])
        .assert()
        .success();

    let id3_out = repo
        .ddb()
        .args([
            "create",
            "--type",
            "foo",
            "--title",
            "Typed note",
            "--set",
            "label=x",
        ])
        .output()
        .unwrap();
    let id3 = String::from_utf8_lossy(&id3_out.stdout).trim().to_string();
    assert_doogat_id(&id3);

    let id1_out = repo
        .ddb()
        .args([
            "create",
            "--title",
            "First note",
            "--tags",
            "test",
            "--body",
            &format!("See [[{id2}|Second note]]."),
        ])
        .output()
        .unwrap();
    let id1 = String::from_utf8_lossy(&id1_out.stdout).trim().to_string();

    // Reindex so the wikilink lands in _ddb_links before `backlinks` can see it.
    repo.ddb().arg("reindex").assert().success();

    repo.ddb()
        .args(["update", &id1, "--title", "First note (edited)"])
        .assert()
        .success();

    repo.ddb()
        .args(["get", &id1])
        .assert()
        .success()
        .stdout(predicate::str::contains("First note (edited)"));

    repo.ddb()
        .args(["scan", "--tag", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains(id1.clone()));

    let scan = repo
        .ddb()
        .args(["scan", "--type", "foo"])
        .assert()
        .success()
        .stdout(predicate::str::contains(id3.clone()));
    assert!(
        String::from_utf8_lossy(&scan.get_output().stdout)
            .lines()
            .any(|line| line == id3),
        "scan --type must print id3 on its own line, got: {:?}",
        String::from_utf8_lossy(&scan.get_output().stdout)
    );

    repo.ddb()
        .args(["backlinks", &id2])
        .assert()
        .success()
        .stdout(predicate::str::contains(id1));
}
