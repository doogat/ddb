//! Cross-process SINGLETON write safety for the UPDATE path (PRD 00157).
//!
//! Two concurrent `ddb update --type app_config` processes retype two distinct
//! base doogats into the same SINGLETON typedef. Exactly one must succeed; the
//! loser must fail with the structured `SINGLETON constraint violated` message
//! naming the surviving row, never a raw SQLite error. Mirrors the create-path
//! race in `singleton_cross_process_create.rs`.
//!
//! On current master `update_doogat_parsed` runs no singleton check on the
//! retype path and has no `BEGIN IMMEDIATE` window, so the loser surfaces a raw
//! materializer error — this test fails RED until the conditional wrap +
//! result-type check land.

use crate::common::{ddb_bin, DdbTestRepo};
use std::process::Stdio;

#[test]
fn two_concurrent_ddb_update_retype_into_singleton_converge_on_one_row() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE app_config (theme TEXT) SINGLETON"])
        .assert()
        .success();

    // Two base (untyped) doogats to retype into the singleton.
    let id_a = create_base(&repo, "Config A");
    let id_b = create_base(&repo, "Config B");

    // Spawn both retype processes before waiting on either — that is the race.
    let child1 = update_child(&repo, &id_a, "dark");
    let child2 = update_child(&repo, &id_b, "light");

    let out1 = child1.wait_with_output().expect("failed to wait on process 1");
    let out2 = child2.wait_with_output().expect("failed to wait on process 2");
    let outputs = [&out1, &out2];

    // Exactly one winner, one loser.
    let success_count = outputs.iter().filter(|o| o.status.success()).count();
    assert_eq!(
        success_count, 1,
        "expected exactly one update to win; status1={}, status2={}",
        out1.status, out2.status
    );
    let failure_count = outputs.iter().filter(|o| !o.status.success()).count();
    assert_eq!(
        failure_count, 1,
        "expected exactly one update to lose; status1={}, status2={}",
        out1.status, out2.status
    );

    // The loser must carry the structured SINGLETON error, not a raw SQL error.
    let loser = outputs.iter().find(|o| !o.status.success()).unwrap();
    let loser_stderr = String::from_utf8_lossy(&loser.stderr);
    let loser_stdout = String::from_utf8_lossy(&loser.stdout);
    assert!(
        loser_stderr.contains("SINGLETON constraint violated: app_config already holds row ")
            || loser_stdout
                .contains("SINGLETON constraint violated: app_config already holds row "),
        "expected structured SINGLETON error; got stderr={loser_stderr} stdout={loser_stdout}"
    );
    assert!(
        !loser_stderr.contains("UNIQUE constraint failed")
            && !loser_stdout.contains("UNIQUE constraint failed"),
        "raw SQLite UNIQUE error must not leak; got stderr={loser_stderr} stdout={loser_stdout}"
    );

    // Exactly one row materialized in the singleton.
    let count = repo
        .ddb()
        .args(["query", "SELECT COUNT(*) AS n FROM app_config"])
        .assert()
        .success();
    let count_stdout = String::from_utf8_lossy(&count.get_output().stdout);
    assert_eq!(count_stdout.trim(), "1", "expected exactly one row");

    // existing_id parity: the loser's error must name the *surviving* row id.
    let surviving = repo
        .ddb()
        .args(["query", "SELECT id FROM app_config"])
        .assert()
        .success();
    let surviving_id = String::from_utf8_lossy(&surviving.get_output().stdout)
        .trim()
        .to_string();
    assert!(
        !surviving_id.is_empty(),
        "surviving singleton row must carry an id"
    );
    assert!(
        loser_stderr.contains(&surviving_id) || loser_stdout.contains(&surviving_id),
        "loser error must name the surviving row id {surviving_id} (existing_id parity); \
         got stderr={loser_stderr} stdout={loser_stdout}"
    );
}

/// Create a base (untyped) doogat and return its id (printed to stdout).
fn create_base(repo: &DdbTestRepo, title: &str) -> String {
    let out = repo
        .ddb()
        .args(["create", "--title", title])
        .assert()
        .success();
    String::from_utf8_lossy(&out.get_output().stdout)
        .trim()
        .to_string()
}

/// Spawn a `ddb update <id> --type app_config --set theme=<theme>` child that
/// races against its sibling for the singleton row.
fn update_child(repo: &DdbTestRepo, id: &str, theme: &str) -> std::process::Child {
    let set_arg = format!("theme={theme}");
    std::process::Command::new(ddb_bin())
        .arg("--repo")
        .arg(repo.path())
        .args(["update", id, "--type", "app_config", "--set", &set_arg])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn update process")
}
