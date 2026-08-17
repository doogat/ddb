//! Ported from tests/integration.sh §33 (lines 3298-3345), byte-stats
//! portion only: `ddb compact --force`'s report includes byte-stats lines.
//! The stale-node-resync flow itself is already covered by
//! `multi_device::stale_node_resync_after_compaction`.

use crate::common::{DdbTestRepo, TwoNodeSetup};
use predicates::prelude::*;

#[test]
fn integration_33_compact_byte_stats_report() {
    let setup = TwoNodeSetup::new();

    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "Stale shared", "--body", "original content"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();

    let node2_path = setup.clone_node2();

    // Both nodes edit the same doogat, producing a CRDT conflict once synced.
    setup
        .node1
        .ddb()
        .args(["update", &id, "--body", "body from node1"])
        .assert()
        .success();
    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["push", "origin", "master"])
        .output()
        .unwrap();

    DdbTestRepo::ddb_at(&node2_path)
        .args(["update", &id, "--body", "body from node2"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&node2_path)
        .args(["sync", "origin", "master"])
        .assert()
        .success();

    DdbTestRepo::ddb_at(&node2_path)
        .args(["compact", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("crdt temp:"))
        .stdout(predicate::str::contains("repo (.git):"));
}
