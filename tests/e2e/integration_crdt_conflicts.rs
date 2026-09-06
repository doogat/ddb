//! CRDT union of two reference-section additions, and PRD
//! 00200's three-way conflicted merge (a deletion and a non-conflicting
//! edit both survive a real conflicted sync).

use crate::common::{DdbTestRepo, TwoNodeSetup};
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

/// Create a doogat via `ddb create` and return its ID.
fn create_doogat(repo: &DdbTestRepo, title: &str, body: &str) -> String {
    let out = repo
        .ddb()
        .args(["create", "--title", title, "--body", body])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn integration_26_reference_section_conflict() {
    let setup = TwoNodeSetup::new();

    let out = setup
        .node1
        .ddb()
        .args(["create", "--title", "Shared note", "--body", "Original body"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();

    let node2_path = setup.clone_node2();

    // node1 syncs to pick up node2's registration before the raw-file edits.
    setup
        .node1
        .ddb()
        .args(["sync", "origin", "master"])
        .assert()
        .success();

    let doogat_rel = format!("ddb/{id}.md");

    // node1: append a laptop-specific reference section, commit, push.
    let node1_file = setup.node1.path().join(&doogat_rel);
    let content = std::fs::read_to_string(&node1_file).unwrap();
    let trimmed = content.trim_end_matches('\n');
    std::fs::write(
        &node1_file,
        format!("{trimmed}\n---\n- laptop note:: Added from laptop\n"),
    )
    .unwrap();
    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["add", &doogat_rel])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["commit", "-m", "node1 add reference"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(setup.node1.path())
        .args(["push", "origin", "master"])
        .output()
        .unwrap();

    // node2: append a different reference section to its own (pre-push)
    // copy of the file, commit, don't push.
    let node2_file = node2_path.join(&doogat_rel);
    let content2 = std::fs::read_to_string(&node2_file).unwrap();
    let trimmed2 = content2.trim_end_matches('\n');
    std::fs::write(
        &node2_file,
        format!("{trimmed2}\n---\n- desktop note:: Added from desktop\n"),
    )
    .unwrap();
    std::process::Command::new("git")
        .current_dir(&node2_path)
        .args(["add", &doogat_rel])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(&node2_path)
        .args(["commit", "-m", "node2 add reference"])
        .output()
        .unwrap();

    DdbTestRepo::ddb_at(&node2_path)
        .args(["sync", "origin", "master"])
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicts resolved: 1"));

    DdbTestRepo::ddb_at(&node2_path)
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("laptop note"))
        .stdout(predicate::str::contains("desktop note"));
}

#[test]
fn integration_27c_three_way_conflicted_merge() {
    let remote_dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(remote_dir.path())
        .output()
        .unwrap();

    // "ours": init, add remote, register, create three doogats, push.
    let ours = DdbTestRepo::init();
    std::process::Command::new("git")
        .current_dir(ours.path())
        .args(["remote", "add", "origin"])
        .arg(remote_dir.path())
        .output()
        .unwrap();
    DdbTestRepo::ddb_at(ours.path())
        .args(["register-node", "Ours"])
        .assert()
        .success();

    let id_a = create_doogat(&ours, "Conflict doogat", "base A");
    let id_x = create_doogat(&ours, "Doomed doogat", "base X");
    let id_y = create_doogat(&ours, "Survivor doogat", "base Y");

    std::process::Command::new("git")
        .current_dir(ours.path())
        .args(["push", "-u", "origin", "master"])
        .output()
        .unwrap();

    // "theirs": clone, reindex, register, edit A + delete X, sync (pushes).
    let theirs_dir = tempfile::TempDir::new().unwrap();
    let theirs_path = theirs_dir.path().join("repo");
    std::process::Command::new("git")
        .args(["clone"])
        .arg(remote_dir.path())
        .arg(&theirs_path)
        .output()
        .unwrap();
    disable_git_signing(&theirs_path);

    DdbTestRepo::ddb_at(&theirs_path)
        .arg("reindex")
        .assert()
        .success();
    DdbTestRepo::ddb_at(&theirs_path)
        .args(["register-node", "Theirs"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&theirs_path)
        .args(["update", &id_a, "--title", "Theirs A"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&theirs_path)
        .args(["delete", &id_x])
        .assert()
        .success();
    DdbTestRepo::ddb_at(&theirs_path)
        .args(["sync", "origin", "master"])
        .assert()
        .success();

    // Back on "ours": collide on A, non-conflicting edit on Y, then sync.
    DdbTestRepo::ddb_at(ours.path())
        .args(["update", &id_a, "--title", "Ours A"])
        .assert()
        .success();
    DdbTestRepo::ddb_at(ours.path())
        .args(["update", &id_y, "--body", "ours Y edit"])
        .assert()
        .success();

    DdbTestRepo::ddb_at(ours.path())
        .args(["sync", "origin", "master"])
        .assert()
        .success()
        .stdout(predicate::str::contains("conflicts resolved: 1"));

    let x_file = ours.path().join(format!("ddb/{id_x}.md"));
    assert!(
        !x_file.exists(),
        "X's deletion should survive the three-way merge"
    );

    DdbTestRepo::ddb_at(ours.path())
        .args(["read", &id_y])
        .assert()
        .success()
        .stdout(predicate::str::contains("ours Y edit"));

    let read_a = DdbTestRepo::ddb_at(ours.path())
        .args(["read", &id_a])
        .output()
        .unwrap();
    let read_a_stdout = String::from_utf8_lossy(&read_a.stdout);
    assert!(
        read_a_stdout.contains("Theirs A") || read_a_stdout.contains("Ours A"),
        "A should resolve to either side's title: {read_a_stdout}"
    );
}
