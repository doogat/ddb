use crate::common::{ddb_bin, DdbTestRepo};
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn stdout(repo: &DdbTestRepo, args: &[&str]) -> String {
    let mut command = repo.ddb();
    let assert = command.args(args).assert().success();
    String::from_utf8_lossy(&assert.get_output().stdout)
        .trim()
        .to_owned()
}

#[test]
fn smoke_01_init() {
    let dir = TempDir::new().unwrap();
    Command::new(ddb_bin())
        .current_dir(dir.path())
        .args(["init", "."])
        .assert()
        .success();
}

#[test]
fn smoke_02_create_unique_ids() {
    let repo = DdbTestRepo::init();
    let id1 = stdout(
        &repo,
        &[
            "create",
            "--title",
            "First note",
            "--tags",
            "test,smoke",
            "--body",
            "Hello world",
        ],
    );
    let id2 = stdout(
        &repo,
        &[
            "create",
            "--title",
            "Links to first",
            "--body",
            &format!("See [[{id1}]]"),
        ],
    );
    let id3 = stdout(
        &repo,
        &[
            "create",
            "--title",
            "Project Alpha",
            "--type",
            "project",
            "--tags",
            "active",
            "--body",
            "A project doogat",
        ],
    );

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);
}

#[test]
fn smoke_03_read() {
    let repo = DdbTestRepo::init();
    let id = stdout(
        &repo,
        &[
            "create",
            "--title",
            "First note",
            "--tags",
            "test,smoke",
            "--body",
            "Hello world",
        ],
    );

    repo.ddb()
        .args(["read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("First note"));
}

#[test]
fn smoke_05_delete() {
    let repo = DdbTestRepo::init();
    let id = stdout(
        &repo,
        &[
            "create",
            "--title",
            "Project Alpha",
            "--body",
            "A project doogat",
        ],
    );

    repo.ddb().args(["delete", &id]).assert().success();
    repo.ddb()
        .args(["read", &id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
    repo.ddb()
        .args(["delete", "99999999999999"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"));
}

#[test]
fn smoke_06_status() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("head:"));
}

#[test]
fn smoke_07_reindex() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "First note"])
        .assert()
        .success();
    repo.ddb()
        .args(["create", "--title", "Second note"])
        .assert()
        .success();

    repo.ddb()
        .arg("reindex")
        .assert()
        .success()
        .stdout(predicate::str::contains("indexed 2 doogats"));
}

#[test]
fn smoke_12_install_bundled_type() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["type", "install", "contact"])
        .assert()
        .success()
        .stdout(predicate::str::contains("installed type"));
}

#[test]
fn smoke_13_type_suggest() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE foo (bar TEXT, baz INTEGER)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO foo (title, bar, baz) VALUES ('for suggest', 'val', 1)",
        ])
        .assert()
        .success();

    repo.ddb()
        .args(["type", "suggest", "foo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bar"));
}

#[test]
fn smoke_14_register_node_and_compact() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "smoke-test-laptop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("registered node"));
    repo.ddb()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("registered nodes: 1"));
    repo.ddb()
        .args(["compact", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("backup:"))
        .stdout(predicate::str::contains("gc: ok"))
        .stdout(predicate::str::contains("crdt temp:"))
        .stdout(predicate::str::contains("repo (.git):"));
}

#[test]
fn smoke_15_node_list_and_retire() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "smoke-test-laptop"])
        .assert()
        .success();
    let nodes = stdout(&repo, &["node", "list"]);
    let node = nodes
        .lines()
        .find(|line| line.contains("smoke-test-laptop"))
        .expect("registered node missing from node list");
    let uuid = node
        .split_whitespace()
        .next()
        .expect("node list row missing UUID");

    repo.ddb()
        .args(["node", "retire", uuid])
        .assert()
        .success()
        .stdout(predicate::str::contains("retired node"));
}

#[test]
fn smoke_16_compact_dry_run() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "dry-run-test"])
        .assert()
        .success();
    repo.ddb()
        .args(["compact", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry run"))
        .stdout(predicate::str::contains("backup would write:"));
}

#[test]
fn smoke_16a_compact_no_backup() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "no-backup-test"])
        .assert()
        .success();

    repo.ddb()
        .args(["compact", "--no-backup", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gc: ok"))
        .stdout(predicate::str::contains("backup:").not());
}

#[test]
fn smoke_16b_compact_backup_path() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "backup-path-test"])
        .assert()
        .success();
    let backup = repo.path().join("custom-backup.bundle.tar");
    let backup_arg = backup.to_string_lossy().into_owned();

    repo.ddb()
        .args(["compact", "--force", "--backup-path", &backup_arg])
        .assert()
        .success()
        .stdout(predicate::str::contains("backup:"))
        .stdout(predicate::str::contains(&backup_arg));
    assert!(backup.is_file(), "custom backup file was not created");
}

#[test]
fn smoke_16c_maintenance() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["maintenance", "run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("maintenance:"));
    repo.ddb()
        .args(["maintenance", "auto", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("off"));
    repo.ddb()
        .args(["maintenance", "auto", "on"])
        .assert()
        .success()
        .stdout(predicate::str::contains("enabled"));
    repo.ddb()
        .args(["maintenance", "auto", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("on"));
    repo.ddb()
        .args(["maintenance", "auto", "off"])
        .assert()
        .success()
        .stdout(predicate::str::contains("disabled"));
    repo.ddb()
        .args(["maintenance", "auto", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("off"));
}

#[test]
fn smoke_16f_log_level_flag() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["--log-level", "debug", "status"])
        .assert()
        .success();
}
