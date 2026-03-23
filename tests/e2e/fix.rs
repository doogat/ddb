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

#[test]
fn fix_verbose_reports_title_noncompliant() {
    let repo = ZdbTestRepo::init();

    // Create typedef with title_template
    repo.zdb()
        .args(["query", "CREATE TABLE widget (name TEXT, description TEXT)"])
        .assert()
        .success();
    repo.zdb()
        .args([
            "query",
            "ALTER TABLE widget SET TITLE TEMPLATE '{name} Widget'",
        ])
        .assert()
        .success();

    // Insert a zettel — its auto-derived title won't match the template
    let out = repo
        .zdb()
        .args([
            "query",
            "INSERT INTO widget (name, description) VALUES ('Foo', 'A foo widget')",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "insert failed: {}", String::from_utf8_lossy(&out.stderr));

    // Verbose dry-run should report title non-compliance
    repo.zdb()
        .args(["fix", "--verbose", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("title does not match template"));
}

#[test]
fn fix_migrate_rewrites_zones() {
    let repo = ZdbTestRepo::init();

    // Create typedef with a TEXT column (defaults to body zone)
    repo.zdb()
        .args([
            "query",
            "CREATE TABLE gadget (name VARCHAR(100), notes TEXT)",
        ])
        .assert()
        .success();

    // Insert zettel — `notes` (TEXT) goes to body zone as ## section
    let out = repo
        .zdb()
        .args([
            "query",
            "INSERT INTO gadget (name, notes) VALUES ('Gizmo', 'Some important notes')",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "insert failed: {}", String::from_utf8_lossy(&out.stderr));
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Verify notes is currently in body zone (## notes section)
    let before = repo.zdb().args(["read", &id]).output().unwrap();
    let before_content = String::from_utf8_lossy(&before.stdout);
    assert!(
        before_content.contains("## notes"),
        "notes should be in body zone before migration: {before_content}"
    );

    // Change zone assignment: move notes to frontmatter
    repo.zdb()
        .args([
            "query",
            "ALTER TABLE gadget SET ZONE frontmatter FOR notes",
        ])
        .assert()
        .success();

    // Run zone migration
    repo.zdb()
        .args(["fix", "--migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("zone-migrated"));

    // Verify notes moved to frontmatter
    let after = repo.zdb().args(["read", &id]).output().unwrap();
    let after_content = String::from_utf8_lossy(&after.stdout);
    assert!(
        after_content.contains("notes: Some important notes"),
        "notes should be in frontmatter after migration: {after_content}"
    );
    assert!(
        !after_content.contains("## notes"),
        "body section should be removed after migration: {after_content}"
    );
}
