//! A raw `git clone`
//! followed by `ddb reindex`, with no `ddb sync` involved at all, already
//! sees content another node pushed.

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
fn integration_21_clone_reindex_reads_pushed_note() {
    let remote_dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(remote_dir.path())
        .output()
        .unwrap();

    // node1: init, add remote, register, create a note, push.
    let node1 = DdbTestRepo::init();
    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["remote", "add", "origin"])
        .arg(remote_dir.path())
        .output()
        .unwrap();
    DdbTestRepo::ddb_at(node1.path())
        .args(["register-node", "Laptop"])
        .assert()
        .success();

    let out = node1
        .ddb()
        .args([
            "create",
            "--title",
            "Shared note",
            "--tags",
            "shared",
            "--body",
            "Original body",
        ])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    std::process::Command::new("git")
        .current_dir(node1.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();

    // node2: raw `git clone` (NOT `ddb sync`), then `ddb reindex` rebuilds
    // the index from the cloned git history alone.
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
        .args(["register-node", "Desktop"])
        .assert()
        .success();

    DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("Shared note"));
}
