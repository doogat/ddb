//! Ports tests/integration.sh §32 (lines 3284-3296): NoSQL CLI commands
//! (get, scan --tag, scan --type, backlinks). The shell script reuses
//! fixture state created much earlier in the script, so this test builds an
//! equivalent self-contained fixture instead.

use crate::common::DdbTestRepo;
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
    assert!(
        id3.len() == 14 && id3.chars().all(|c| c.is_ascii_digit()),
        "typed fixture must return a 14-digit id, got: {id3:?}"
    );

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
        .stdout(predicate::str::contains(id3));
    assert!(
        String::from_utf8_lossy(&scan.get_output().stdout)
            .lines()
            .any(|line| line.len() == 14 && line.chars().all(|c| c.is_ascii_digit())),
        "scan --type must print a 14-digit id on its own line"
    );

    repo.ddb()
        .args(["backlinks", &id2])
        .assert()
        .success()
        .stdout(predicate::str::contains(id1));
}
