use assert_cmd::Command;
use predicates::prelude::*;

fn ddb() -> Command {
    Command::new(crate::common::ddb_bin())
}

#[test]
fn help_create_app_prints_guide() {
    ddb()
        .args(["help", "create-app"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CREATE TABLE"))
        .stdout(predicate::str::contains("zone"));
}

#[test]
fn help_unknown_topic_fails() {
    ddb()
        .args(["help", "nonexistent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown guide"));
}

#[test]
fn help_no_topic_lists_guides() {
    ddb()
        .args(["help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("create-app"));
}

#[test]
fn query_long_help_mentions_guide() {
    ddb()
        .args(["query", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ddb help create-app"));
}

#[test]
fn get_help_does_not_claim_nosql_index() {
    ddb()
        .args(["get", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("NoSQL index").not());
}

#[test]
fn backlinks_help_does_not_claim_nosql_index() {
    ddb()
        .args(["backlinks", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("NoSQL index").not());
}

#[test]
fn scan_help_still_mentions_nosql_index() {
    ddb()
        .args(["scan", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("NoSQL index"));
}
