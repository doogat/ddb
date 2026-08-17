use crate::common::DdbTestRepo;
use predicates::prelude::*;

const POISON_PATH: &str = "ddb/29990101000000.md";
const POISON_PATH2: &str = "ddb/29990101000001.md";
const POISON_CONTENT: &str = "---\n: invalid yaml [\n---\nbody\n";

#[test]
fn smoke_32_cli_answers_after_strict_abort() {
    let repo = DdbTestRepo::init();
    let mut create = repo.ddb();
    let create = create
        .args(["create", "--title", "First note"])
        .assert()
        .success();
    let id = String::from_utf8_lossy(&create.get_output().stdout)
        .trim()
        .to_owned();
    std::fs::write(repo.path().join(POISON_PATH), POISON_CONTENT).unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-m", "add poison doogat"][..],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .status()
            .expect("git failed to run");
        assert!(status.success(), "git {args:?} failed");
    }

    repo.ddb()
        .args(["reindex", "--strict"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("error:"))
        .stderr(predicate::str::contains(POISON_PATH));
    repo.ddb()
        .args([
            "query",
            &format!("SELECT id FROM doogats WHERE id = '{id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id));
}

#[test]
fn smoke_33_reindex_report_and_create_warning() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "Before poison"])
        .assert()
        .success();
    std::fs::write(repo.path().join(POISON_PATH), POISON_CONTENT).unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-m", "add poison doogat"][..],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .status()
            .expect("git failed to run");
        assert!(status.success(), "git {args:?} failed");
    }

    repo.ddb()
        .env("RUST_LOG", "error")
        .arg("reindex")
        .assert()
        .success()
        .stdout(predicate::str::contains(POISON_PATH));

    std::fs::write(repo.path().join(POISON_PATH2), POISON_CONTENT).unwrap();
    for args in [
        &["add", "-A"][..],
        &["commit", "-m", "add second poison doogat"][..],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .status()
            .expect("git failed to run");
        assert!(status.success(), "git {args:?} failed");
    }

    repo.ddb()
        .args([
            "create",
            "--title",
            "After poison",
            "--body",
            "triggers freshness reindex",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("REINDEX_SKIPPED_FILES"));
}
