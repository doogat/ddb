use crate::common::DdbTestRepo;
use predicates::prelude::*;

/// Create a doogat with nested YAML frontmatter, then verify that
/// dot-notation keys are flattened into the `_ddb_fields` index.
#[test]
fn nested_frontmatter_indexed_with_dot_notation() {
    let repo = DdbTestRepo::init();

    // Create a doogat, get its ID
    let out = repo
        .ddb()
        .args(["create", "--title", "Nested Test"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Overwrite the doogat file with nested frontmatter
    let doogat_path = repo.path().join(format!("ddb/{id}.md"));
    let content = format!(
        "---\n\
         id: {id}\n\
         title: Nested Test\n\
         author:\n\
         \x20 name: Alice\n\
         \x20 email: alice@example.com\n\
         scores:\n\
         \x20 - 10\n\
         \x20 - 20\n\
         ---\n\
         \n\
         Body content here.\n"
    );
    std::fs::write(&doogat_path, &content).unwrap();

    // Git-commit the change so the index sees it
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "update", "--allow-empty"])
        .current_dir(repo.path())
        .output()
        .unwrap();

    // Force reindex
    repo.ddb().arg("reindex").assert().success();

    // Query _ddb_fields for dot-notation keys
    repo.ddb()
        .args([
            "query",
            &format!(
                "SELECT key, value FROM _ddb_fields WHERE doogat_id = '{id}' AND key LIKE 'author.%' ORDER BY key"
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("author.email"))
        .stdout(predicate::str::contains("alice@example.com"))
        .stdout(predicate::str::contains("author.name"))
        .stdout(predicate::str::contains("Alice"));

    // Query for list items with bracket notation
    repo.ddb()
        .args([
            "query",
            &format!(
                "SELECT key, value FROM _ddb_fields WHERE doogat_id = '{id}' AND key LIKE 'scores%' ORDER BY key"
            ),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("scores[0]"))
        .stdout(predicate::str::contains("10"))
        .stdout(predicate::str::contains("scores[1]"))
        .stdout(predicate::str::contains("20"));
}
