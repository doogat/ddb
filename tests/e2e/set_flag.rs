use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn create_with_set_on_typed_doogat() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["type", "install", "contact"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let out = repo
        .ddb()
        .args([
            "create",
            "--type",
            "contact",
            "--title",
            "Alice",
            "--set",
            "email=alice@example.com",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("email: alice@example.com"));
}

#[test]
fn update_set_adds_field() {
    let repo = DdbTestRepo::init();
    let out = repo
        .ddb()
        .args(["create", "--title", "TaskItem"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["update", &id, "--set", "status=done"])
        .assert()
        .success();

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("status: done"));
}

#[test]
fn update_unset_removes_field() {
    let repo = DdbTestRepo::init();
    let out = repo
        .ddb()
        .args(["create", "--title", "Removable", "--set", "status=active"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Confirm the field exists
    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("status: active"));

    repo.ddb()
        .args(["update", &id, "--unset", "status"])
        .assert()
        .success();

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("status:").not());
}

#[test]
fn create_with_multiple_set_flags() {
    let repo = DdbTestRepo::init();
    let out = repo
        .ddb()
        .args([
            "create", "--title", "Multi", "--set", "a=1", "--set", "b=2",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("a: '1'"))
        .stdout(predicate::str::contains("b: '2'"));
}

#[test]
fn create_with_malformed_set_returns_error() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "Bad", "--set", "noequals"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid --set format"));
}
