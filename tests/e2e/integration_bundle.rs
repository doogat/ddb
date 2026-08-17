//! Ported from tests/integration.sh §28 (lines 3181-3191): a targeted
//! `bundle export --target <uuid>` delta export (PRD 00168 bundle import),
//! distinct from `--full`, delivers content scoped to the named node.

use crate::common::{DdbTestRepo, TwoNodeSetup};
use predicates::prelude::*;

#[test]
fn integration_28_delta_bundle_target_export() {
    let setup = TwoNodeSetup::new();

    // node1 creates a note and pushes so node2 has something to clone.
    setup
        .node1
        .ddb()
        .args(["create", "--title", "Seed note", "--body", "seed"])
        .assert()
        .success();

    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();

    let node2_path = setup.clone_node2();

    // node2 syncs to push its node registration to origin, then node1 syncs
    // to learn about node2 as a valid `--target` for a delta bundle export.
    DdbTestRepo::ddb_at(&node2_path)
        .args(["sync", "origin", "master"])
        .assert()
        .success();
    setup
        .node1
        .ddb()
        .args(["sync", "origin", "master"])
        .assert()
        .success();

    // node1 creates the doogat that will be delta-exported to node2.
    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "Delta note", "--body", "only in delta"])
        .output()
        .unwrap();
    let delta_id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let node2_uuid = std::fs::read_to_string(node2_path.join(".git/ddb-node"))
        .unwrap()
        .trim()
        .to_string();

    let tmp = tempfile::TempDir::new().unwrap();
    let bundle_path = tmp.path().join("delta-bundle.tar");

    setup
        .node1
        .ddb()
        .args(["bundle", "export", "--target", &node2_uuid, "--output"])
        .arg(&bundle_path)
        .assert()
        .success();

    DdbTestRepo::ddb_at(&node2_path)
        .args(["bundle", "import"])
        .arg(&bundle_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("imported"));

    DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &delta_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("Delta note"));
}
