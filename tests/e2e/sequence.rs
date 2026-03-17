use crate::common::ZdbTestRepo;
use predicates::prelude::*;

/// Helper: create a zettel, patch its frontmatter to include `sequence: <parent_id>`,
/// commit and reindex.
fn create_with_sequence(repo: &ZdbTestRepo, title: &str, parent_id: &str) -> String {
    let out = repo
        .zdb()
        .args(["create", "--title", title])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let zettel_path = repo.path().join(format!("zettelkasten/{id}.md"));
    let content = format!(
        "---\n\
         id: {id}\n\
         title: {title}\n\
         sequence: {parent_id}\n\
         ---\n\n"
    );
    std::fs::write(&zettel_path, &content).unwrap();

    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "add sequence field"])
        .current_dir(repo.path())
        .output()
        .unwrap();

    id
}

#[test]
fn sequence_tree_display() {
    let repo = ZdbTestRepo::init();

    // Create root zettel
    let out = repo
        .zdb()
        .args(["create", "--title", "Root Note"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let root_id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Create two children
    let child1_id = create_with_sequence(&repo, "Child One", &root_id);
    let child2_id = create_with_sequence(&repo, "Child Two", &root_id);

    repo.zdb().arg("reindex").assert().success();

    // Verify tree shows children
    repo.zdb()
        .args(["sequence", "tree", &root_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(&child1_id))
        .stdout(predicate::str::contains(&child2_id));
}

#[test]
fn sequence_breadcrumb_display() {
    let repo = ZdbTestRepo::init();

    // Create 3-level chain: root → mid → leaf
    let out = repo
        .zdb()
        .args(["create", "--title", "Root"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let root_id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let mid_id = create_with_sequence(&repo, "Mid", &root_id);
    let leaf_id = create_with_sequence(&repo, "Leaf", &mid_id);

    repo.zdb().arg("reindex").assert().success();

    // Verify breadcrumb shows root, mid, leaf in order
    repo.zdb()
        .args(["sequence", "breadcrumb", &leaf_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(&root_id))
        .stdout(predicate::str::contains(&mid_id))
        .stdout(predicate::str::contains(&leaf_id));
}

#[test]
fn sequence_broken_list() {
    let repo = ZdbTestRepo::init();

    let broken_id = create_with_sequence(&repo, "Broken Child", "99999999999999");

    repo.zdb().arg("reindex").assert().success();

    repo.zdb()
        .args(["sequence", "broken"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&broken_id))
        .stdout(predicate::str::contains("99999999999999"))
        .stdout(predicate::str::contains("not found"));
}
