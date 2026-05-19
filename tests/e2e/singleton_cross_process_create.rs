//! Cross-process SINGLETON write safety (PRD 00140).
//!
//! Two concurrent `ddb create` processes race against the same SINGLETON
//! typedef. Exactly one must succeed; the loser must fail with the structured
//! `SINGLETON constraint violated` message rather than a raw SQLite error.

use crate::common::{DdbTestRepo, ddb_bin};
use std::process::Stdio;

#[test]
fn two_concurrent_ddb_create_on_singleton_converge_on_one_row() {
    let repo = DdbTestRepo::init();

    // Define SINGLETON typedef.
    repo.ddb()
        .args(["query", "CREATE TABLE app_config (theme TEXT) SINGLETON"])
        .assert()
        .success();

    // Spawn both processes before waiting on either — that is the race.
    let child1 = std::process::Command::new(ddb_bin())
        .arg("--repo")
        .arg(repo.path())
        .args(["create", "--type", "app_config", "--title", "Config", "--set", "theme=dark"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn process 1");

    let child2 = std::process::Command::new(ddb_bin())
        .arg("--repo")
        .arg(repo.path())
        .args(["create", "--type", "app_config", "--title", "Config", "--set", "theme=dark"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn process 2");

    let out1 = child1.wait_with_output().expect("failed to wait on process 1");
    let out2 = child2.wait_with_output().expect("failed to wait on process 2");

    let outputs = [&out1, &out2];

    // Exactly one winner, one loser.
    let success_count = outputs.iter().filter(|o| o.status.success()).count();
    let failure_count = outputs.iter().filter(|o| !o.status.success()).count();
    assert_eq!(
        success_count, 1,
        "expected exactly one process to succeed; status1={}, status2={}",
        out1.status, out2.status
    );
    assert_eq!(
        failure_count, 1,
        "expected exactly one process to fail; status1={}, status2={}",
        out1.status, out2.status
    );

    // The loser must carry the structured SINGLETON error.
    let loser = outputs.iter().find(|o| !o.status.success()).unwrap();
    let loser_stderr = String::from_utf8_lossy(&loser.stderr);
    let loser_stdout = String::from_utf8_lossy(&loser.stdout);
    assert!(
        loser_stderr.contains("SINGLETON constraint violated: app_config already holds row ")
            || loser_stdout.contains("SINGLETON constraint violated: app_config already holds row "),
        "expected structured SINGLETON error; got stderr={loser_stderr} stdout={loser_stdout}"
    );
    assert!(
        !loser_stderr.contains("UNIQUE constraint failed")
            && !loser_stdout.contains("UNIQUE constraint failed"),
        "raw SQLite UNIQUE error must not leak; got stderr={loser_stderr} stdout={loser_stdout}"
    );

    // Exactly one row materialized.
    repo.ddb()
        .args(["query", "SELECT * FROM app_config"])
        .assert()
        .success()
        .stdout(predicates::str::contains("dark"));

    // Exactly one row — count must be 1, not just "a row with dark exists".
    repo.ddb()
        .args(["query", "SELECT COUNT(*) AS n FROM app_config"])
        .assert()
        .success()
        .stdout(predicates::str::contains("1"));
}
