use assert_cmd::Command;
use predicates::prelude::*;

fn zdb() -> Command {
    Command::new(crate::common::zdb_bin())
}

#[test]
fn help_create_app_prints_guide() {
    zdb()
        .args(["help", "create-app"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CREATE TABLE"))
        .stdout(predicate::str::contains("zone"));
}

#[test]
fn help_unknown_topic_fails() {
    zdb()
        .args(["help", "nonexistent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown guide"));
}

#[test]
fn help_no_topic_lists_guides() {
    zdb()
        .args(["help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("create-app"));
}

#[test]
fn query_long_help_mentions_guide() {
    zdb()
        .args(["query", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("zdb help create-app"));
}
