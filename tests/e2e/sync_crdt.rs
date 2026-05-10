use crate::common::{DdbTestRepo, TwoNodeSetup};
use predicates::prelude::*;

/// Helper: push node1's master to remote
fn push_node1(setup: &TwoNodeSetup) {
    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();
}

#[test]
fn two_node_fast_forward_sync() {
    let setup = TwoNodeSetup::new();

    // Create note on node1, push
    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "Note One", "--body", "hello"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    push_node1(&setup);

    // Clone node2, sync
    let node2_path = setup.clone_node2();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Node2 can read the note
    DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("Note One"))
        .stdout(predicate::str::contains("hello"));
}

#[test]
fn non_overlapping_edits_merge_cleanly() {
    let setup = TwoNodeSetup::new();

    // Shared note
    let out = setup
        .node1
        .ddb()
        .args([
            "create",
            "--title",
            "Original",
            "--tags",
            "a",
            "--body",
            "original body",
        ])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    push_node1(&setup);
    let node2_path = setup.clone_node2();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Node1: edit frontmatter (title + tags)
    setup
        .node1
        .ddb()
        .args(["update", &id, "--title", "Updated Title", "--tags", "a,b"])
        .assert()
        .success();

    // Node2: edit body
    DdbTestRepo::ddb_at(&node2_path)
        .args(["update", &id, "--body", "modified body"])
        .assert()
        .success();

    // Node1 syncs first
    setup.node1.ddb().arg("sync").assert().success();

    // Node2 syncs — should merge without conflict
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicts resolved: 0"));

    // Merged result has both changes
    DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated Title"))
        .stdout(predicate::str::contains("- b"))
        .stdout(predicate::str::contains("modified body"));
}

#[test]
fn overlapping_body_edits_resolved_by_crdt() {
    let setup = TwoNodeSetup::new();

    let out = setup
        .node1
        .ddb()
        .args([
            "create",
            "--title",
            "Doc",
            "--body",
            "Architecture overview.\nFrontend: React.\nBackend: Rust.",
        ])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    push_node1(&setup);
    let node2_path = setup.clone_node2();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Both edit the same line
    setup
        .node1
        .ddb()
        .args([
            "update",
            &id,
            "--body",
            "Architecture overview — LAPTOP.\nFrontend: React.\nBackend: Rust.",
        ])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&node2_path)
        .args([
            "update",
            &id,
            "--body",
            "Architecture overview — DESKTOP.\nFrontend: React.\nBackend: Rust.",
        ])
        .assert()
        .success();

    // Node1 pushes, node2 syncs
    setup.node1.ddb().arg("sync").assert().success();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicts resolved: 1"));

    // Both fragments present in merged result
    let result = DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .output()
        .unwrap();
    let body = String::from_utf8_lossy(&result.stdout);
    assert!(body.contains("LAPTOP"), "merged body should contain LAPTOP");
    assert!(
        body.contains("DESKTOP"),
        "merged body should contain DESKTOP"
    );
    assert!(
        body.contains("Frontend: React."),
        "unchanged lines preserved"
    );
}

#[test]
fn frontmatter_field_conflict_resolved_by_crdt() {
    let setup = TwoNodeSetup::new();

    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "Shared", "--tags", "original"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    push_node1(&setup);
    let node2_path = setup.clone_node2();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Both change tags
    setup
        .node1
        .ddb()
        .args(["update", &id, "--tags", "laptop-tag"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&node2_path)
        .args(["update", &id, "--tags", "desktop-tag"])
        .assert()
        .success();

    setup.node1.ddb().arg("sync").assert().success();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicts resolved: 1"));

    // Title and other fields preserved regardless of which tag wins
    DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("title: Shared"));
}

#[test]
fn nodes_converge_after_sync() {
    let setup = TwoNodeSetup::new();

    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "Converge Test", "--body", "original"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    push_node1(&setup);
    let node2_path = setup.clone_node2();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Diverge
    setup
        .node1
        .ddb()
        .args(["update", &id, "--title", "From Laptop"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&node2_path)
        .args(["update", &id, "--body", "desktop body"])
        .assert()
        .success();

    // Sync both directions
    setup.node1.ddb().arg("sync").assert().success();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();
    setup.node1.ddb().arg("sync").assert().success();

    // Both should return identical content
    let r1 = setup.node1.ddb().args(["read", &id]).output().unwrap();
    let r2 = DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .output()
        .unwrap();
    assert_eq!(r1.stdout, r2.stdout, "both nodes should converge");
}

#[test]
fn concurrent_tag_additions_both_survive() {
    let setup = TwoNodeSetup::new();

    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "TagTest", "--tags", "shared"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    push_node1(&setup);
    let node2_path = setup.clone_node2();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Node1 adds tag "laptop"
    setup
        .node1
        .ddb()
        .args(["update", &id, "--tags", "shared,laptop"])
        .assert()
        .success();

    // Node2 adds tag "desktop"
    DdbTestRepo::ddb_at(&node2_path)
        .args(["update", &id, "--tags", "shared,desktop"])
        .assert()
        .success();

    // Sync
    setup.node1.ddb().arg("sync").assert().success();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Both tags should be present after merge
    let result = DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .output()
        .unwrap();
    let content = String::from_utf8_lossy(&result.stdout);
    assert!(content.contains("shared"), "shared tag preserved");
    assert!(content.contains("laptop"), "laptop tag from node1 present");
    assert!(
        content.contains("desktop"),
        "desktop tag from node2 present"
    );
}

#[test]
fn frontmatter_conflict_creates_fm_crdt_file() {
    let setup = TwoNodeSetup::new();

    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "FmCrdt", "--tags", "base"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    push_node1(&setup);
    let node2_path = setup.clone_node2();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success();

    // Both change frontmatter (title) to create a conflict
    setup
        .node1
        .ddb()
        .args(["update", &id, "--title", "Laptop Title"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&node2_path)
        .args(["update", &id, "--title", "Desktop Title"])
        .assert()
        .success();

    // Node1 pushes, node2 syncs → conflict resolution
    setup.node1.ddb().arg("sync").assert().success();
    DdbTestRepo::ddb_at(&node2_path)
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicts resolved: 1"));

    // Check _fm.crdt file exists in node2's .crdt/temp/
    let temp_dir = node2_path.join(".crdt/temp");
    let fm_files: Vec<_> = std::fs::read_dir(&temp_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with("_fm.crdt"))
        .collect();
    assert!(
        !fm_files.is_empty(),
        "_fm.crdt file should exist after frontmatter conflict resolution"
    );

    // Verify the _fm.crdt file contains valid automerge data
    let data = std::fs::read(fm_files[0].path()).unwrap();
    assert!(!data.is_empty(), "_fm.crdt file should not be empty");

    // Run compaction — should handle _fm.crdt files
    DdbTestRepo::ddb_at(&node2_path)
        .args(["compact", "--force"])
        .assert()
        .success();
}

#[test]
fn sync_auto_registers_node_when_none_exists() {
    let remote_dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(remote_dir.path())
        .output()
        .unwrap();

    let node = DdbTestRepo::init();
    std::process::Command::new("git")
        .current_dir(node.path())
        .args(["remote", "add", "origin"])
        .arg(remote_dir.path())
        .output()
        .unwrap();

    // Push initial commit so remote has a master branch
    std::process::Command::new("git")
        .current_dir(node.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();

    // No register-node — sync should auto-register
    let node_file = node.path().join(".git/ddb-node");
    assert!(!node_file.exists());

    node.ddb().arg("sync").assert().success();
    assert!(
        node_file.exists(),
        "auto-register should create .git/ddb-node"
    );

    // Subsequent sync should reuse registration
    node.ddb().arg("sync").assert().success();
}
