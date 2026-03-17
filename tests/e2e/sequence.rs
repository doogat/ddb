use crate::common::{ServerGuard, ZdbTestRepo};
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

#[test]
fn sequence_graphql_queries() {
    let repo = ZdbTestRepo::init();

    // Create root
    let out = repo
        .zdb()
        .args(["create", "--title", "GQL Root"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let root_id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Create child with sequence field
    let child_id = create_with_sequence(&repo, "GQL Child", &root_id);
    repo.zdb().arg("reindex").assert().success();

    let server = ServerGuard::start(&repo);

    // sequenceChildren
    let result = server.graphql(&format!(
        r#"{{ sequenceChildren(id: "{root_id}") {{ id title }} }}"#
    ));
    let children = &result["data"]["sequenceChildren"];
    assert!(children.is_array());
    assert_eq!(children[0]["id"].as_str().unwrap(), child_id);

    // sequenceBreadcrumb
    let result = server.graphql(&format!(
        r#"{{ sequenceBreadcrumb(id: "{child_id}") {{ id title }} }}"#
    ));
    let bc = &result["data"]["sequenceBreadcrumb"];
    assert!(bc.is_array());
    assert_eq!(bc[0]["id"].as_str().unwrap(), root_id);
    assert_eq!(bc[1]["id"].as_str().unwrap(), child_id);

    // sequenceInfo
    let result = server.graphql(&format!(
        r#"{{ sequenceInfo(id: "{child_id}") {{ parent {{ id }} children {{ id }} breadcrumb {{ id }} }} }}"#
    ));
    let info = &result["data"]["sequenceInfo"];
    assert_eq!(info["parent"]["id"].as_str().unwrap(), root_id);

    // brokenSequences (should be empty here)
    let result = server.graphql(r#"{ brokenSequences { zettelId brokenParentId } }"#);
    let broken = &result["data"]["brokenSequences"];
    assert!(broken.is_array());
    assert_eq!(broken.as_array().unwrap().len(), 0);
}
