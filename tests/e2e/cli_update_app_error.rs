use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn cli_update_missing_id_reports_app_error() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["update", "99999999999999", "--title", "Whatever"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn cli_update_existing_prints_updated_and_changes_title() {
    let repo = DdbTestRepo::init();
    let out = repo
        .ddb()
        .args(["create", "--title", "Original"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["update", &id, "--title", "Renamed"])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated"))
        .stdout(predicate::str::contains(&id));

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("Renamed"));
}
