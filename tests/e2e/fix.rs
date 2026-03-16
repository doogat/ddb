use crate::common::ZdbTestRepo;
use predicates::prelude::*;

#[test]
fn fix_dry_run_no_changes() {
    let repo = ZdbTestRepo::init();

    // Create zettel with unsorted tags
    let out = repo
        .zdb()
        .args(["create", "--title", "Test", "--tags", "zebra,apple"])
        .output()
        .unwrap();
    let _id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Count commits before fix
    let log_before = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .unwrap();
    let count_before = String::from_utf8_lossy(&log_before.stdout)
        .trim()
        .to_string();

    // Dry run
    repo.zdb()
        .args(["fix", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("would fix"));

    // Commit count unchanged
    let log_after = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .unwrap();
    let count_after = String::from_utf8_lossy(&log_after.stdout)
        .trim()
        .to_string();
    assert_eq!(
        count_before, count_after,
        "dry run should not create commits"
    );
}

#[test]
fn fix_commits_changes() {
    let repo = ZdbTestRepo::init();

    // Create zettel with hash-prefixed tag
    let out = repo
        .zdb()
        .args(["create", "--title", "Test", "--tags", "#gtd,work"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Apply fixes
    repo.zdb()
        .args(["fix"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fixed"));

    // Read back and verify tag is normalized
    let read_out = repo.zdb().args(["read", &id]).output().unwrap();
    let content = String::from_utf8_lossy(&read_out.stdout);
    assert!(
        content.contains("  - gtd"),
        "tag should be stripped of #: {content}"
    );
    assert!(
        !content.contains("#gtd"),
        "hash-prefixed tag should be gone: {content}"
    );
}

#[test]
fn fix_idempotent() {
    let repo = ZdbTestRepo::init();

    // Create zettel with issues
    repo.zdb()
        .args(["create", "--title", "Test", "--tags", "zebra,apple,apple"])
        .assert()
        .success();

    // First fix
    repo.zdb()
        .args(["fix"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fixed"));

    // Second fix — should find nothing
    repo.zdb()
        .args(["fix"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no issues found"));
}

#[test]
fn fix_verbose_output() {
    let repo = ZdbTestRepo::init();

    // Create zettel with multiple issues
    repo.zdb()
        .args(["create", "--title", "Test", "--tags", "#gtd,zebra,apple"])
        .assert()
        .success();

    // Verbose dry run
    repo.zdb()
        .args(["fix", "--dry-run", "--verbose"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[info]"));
}
