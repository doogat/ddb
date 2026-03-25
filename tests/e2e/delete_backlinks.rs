use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn delete_reports_broken_backlinks() {
    let repo = DdbTestRepo::init();

    // Create target doogat A
    let a_out = repo
        .ddb()
        .args(["create", "--title", "Target"])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create doogat B that links to A
    repo.ddb()
        .args([
            "create",
            "--title",
            "Linker",
            "--body",
            &format!("See [[{a_id}|Target]]."),
        ])
        .assert()
        .success();

    // Reindex so wikilinks are in _ddb_links
    repo.ddb().arg("reindex").assert().success();

    // Delete A — should warn about B's broken backlink
    repo.ddb()
        .args(["delete", &a_id])
        .assert()
        .success()
        .stderr(predicate::str::contains("broken backlinks"));
}

#[test]
fn status_reports_broken_backlinks_after_delete() {
    let repo = DdbTestRepo::init();

    // Create target doogat A
    let a_out = repo
        .ddb()
        .args(["create", "--title", "Target"])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create doogat B that links to A
    repo.ddb()
        .args([
            "create",
            "--title",
            "Linker",
            "--body",
            &format!("See [[{a_id}]]."),
        ])
        .assert()
        .success();

    // Reindex so wikilinks are in _ddb_links
    repo.ddb().arg("reindex").assert().success();

    // Delete A
    repo.ddb().args(["delete", &a_id]).assert().success();

    // Status should report broken backlinks
    repo.ddb()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("broken backlinks"));
}

#[test]
fn delete_no_backlinks_no_warning() {
    let repo = DdbTestRepo::init();

    let out = repo
        .ddb()
        .args(["create", "--title", "Lonely"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["delete", &id])
        .assert()
        .success()
        .stderr(predicate::str::contains("broken backlinks").not());
}
