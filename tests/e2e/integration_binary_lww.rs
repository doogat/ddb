//! Ported from tests/integration.sh §40 (lines 3407-3460): a binary asset
//! conflict resolves via last-writer-wins on the commit's `HLC:` trailer
//! (PRD 00166), preserving the loser's content in history via a merge
//! commit rather than silently dropping it.

use crate::common::DdbTestRepo;
use predicates::prelude::*;

/// Disable git commit signing for a freshly cloned repo. Mirrors the
/// private `DdbTestRepo::disable_git_signing`, which this module can't call.
fn disable_git_signing(path: &std::path::Path) {
    std::process::Command::new("git")
        .current_dir(path)
        .args(["config", "commit.gpgsign", "false"])
        .status()
        .unwrap();
}

#[test]
fn integration_40_binary_asset_lww() {
    let remote_dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(remote_dir.path())
        .output()
        .unwrap();

    // node1: init, add remote, register, create a doogat, add a binary asset, push.
    let node1 = DdbTestRepo::init();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["remote", "add", "origin"])
        .arg(remote_dir.path())
        .output()
        .unwrap();
    DdbTestRepo::ddb_at(node1.path())
        .args(["register-node", "BinNode1"])
        .assert()
        .success();
    node1
        .ddb()
        .args(["create", "--title", "Binary test"])
        .assert()
        .success();

    let asset_rel = "reference/test/photo.bin";
    let asset_path = node1.path().join(asset_rel);
    std::fs::create_dir_all(asset_path.parent().unwrap()).unwrap();
    std::fs::write(&asset_path, b"\x89PNG\r\n").unwrap();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["add", asset_rel])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["commit", "-m", "add binary asset"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();

    // node2: raw clone, reindex, register.
    let node2_dir = tempfile::TempDir::new().unwrap();
    let node2_path = node2_dir.path().join("repo");
    std::process::Command::new("git")
        .args(["clone"])
        .arg(remote_dir.path())
        .arg(&node2_path)
        .output()
        .unwrap();
    disable_git_signing(&node2_path);
    DdbTestRepo::ddb_at(&node2_path)
        .arg("reindex")
        .assert()
        .success();
    DdbTestRepo::ddb_at(&node2_path)
        .args(["register-node", "BinNode2"])
        .assert()
        .success();

    // node1: overwrite the binary with a HIGHER HLC wall_ms, push.
    std::fs::write(&asset_path, b"NODE1_WINS_CONTENT").unwrap();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["add", asset_rel])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args([
            "commit",
            "-m",
            "node1 update binary\n\nHLC: 9999999999999-0000-BinNode1",
        ])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["push", "origin", "master"])
        .output()
        .unwrap();

    // node2: overwrite the same binary with a LOWER HLC wall_ms, don't push.
    let node2_asset_path = node2_path.join(asset_rel);
    std::fs::write(&node2_asset_path, b"NODE2_LOSES_CONTENT").unwrap();
    std::process::Command::new("git")
        .current_dir(&node2_path)
        .args(["add", asset_rel])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(&node2_path)
        .args([
            "commit",
            "-m",
            "node2 update binary\n\nHLC: 1000000000000-0000-BinNode2",
        ])
        .output()
        .unwrap();

    DdbTestRepo::ddb_at(&node2_path)
        .args(["sync", "origin", "master"])
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicts resolved: 1"));

    let resolved = std::fs::read_to_string(&node2_asset_path).unwrap();
    assert_eq!(resolved, "NODE1_WINS_CONTENT");

    let merge_log = String::from_utf8(
        std::process::Command::new("git")
            .current_dir(&node2_path)
            .args(["log", "--merges", "--oneline", "-1"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        merge_log.contains("resolve merge"),
        "merge log: {merge_log}"
    );
}
