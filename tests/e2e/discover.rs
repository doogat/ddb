use crate::common::ZdbTestRepo;
use predicates::prelude::*;

#[test]
fn discover_mentions() {
    let repo = ZdbTestRepo::init();

    // Create zettel A with a distinctive title
    let a_out = repo
        .zdb()
        .args([
            "create",
            "--title",
            "Meeting Notes",
            "--body",
            "Agenda items.",
        ])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create zettel B whose body mentions "Meeting Notes" but doesn't link to A
    let b_out = repo
        .zdb()
        .args([
            "create",
            "--title",
            "Follow-up",
            "--body",
            "Discussed in Meeting Notes yesterday.",
        ])
        .output()
        .unwrap();
    let b_id = String::from_utf8_lossy(&b_out.stdout).trim().to_string();

    // Reindex so FTS picks up the body text
    repo.zdb().arg("reindex").assert().success();

    // discover mentions should find B as an unlinked mention of A
    repo.zdb()
        .args(["discover", "mentions", &a_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(&b_id));
}

#[test]
fn discover_mentions_excludes_linked() {
    let repo = ZdbTestRepo::init();

    // Create zettel A
    let a_out = repo
        .zdb()
        .args(["create", "--title", "Meeting Notes", "--body", "Agenda."])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create zettel C that mentions "Meeting Notes" AND links to A
    repo.zdb()
        .args([
            "create",
            "--title",
            "Linked Follow-up",
            "--body",
            &format!("See Meeting Notes at [[{a_id}]]."),
        ])
        .assert()
        .success();

    repo.zdb().arg("reindex").assert().success();

    // C should NOT appear because it already links to A
    repo.zdb()
        .args(["discover", "mentions", &a_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("no unlinked mentions"));
}

#[test]
fn discover_similar() {
    let repo = ZdbTestRepo::init();

    // Create two zettels with shared tags
    let a_out = repo
        .zdb()
        .args([
            "create",
            "--title",
            "Rust Intro",
            "--tags",
            "rust,programming",
            "--body",
            "Getting started with Rust.",
        ])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    repo.zdb()
        .args([
            "create",
            "--title",
            "Rust Advanced",
            "--tags",
            "rust,programming",
            "--body",
            "Advanced Rust patterns.",
        ])
        .assert()
        .success();

    repo.zdb().arg("reindex").assert().success();

    // discover similar should return at least one result (not "no suggestions")
    repo.zdb()
        .args(["discover", "similar", &a_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("no suggestions").not())
        .stdout(predicate::str::contains("Rust Advanced"));
}

#[test]
fn discover_orphans() {
    let repo = ZdbTestRepo::init();

    // Create zettel with no incoming links
    let out = repo
        .zdb()
        .args([
            "create",
            "--title",
            "Island Note",
            "--body",
            "Nobody links to me.",
        ])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.zdb().arg("reindex").assert().success();

    // discover orphans should find it
    repo.zdb()
        .args(["discover", "orphans"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id))
        .stdout(predicate::str::contains("Island Note"));
}

#[test]
fn discover_orphans_excludes_linked() {
    let repo = ZdbTestRepo::init();

    // Create target zettel
    let a_out = repo
        .zdb()
        .args(["create", "--title", "Target", "--body", "I am linked."])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create zettel that links to target
    repo.zdb()
        .args([
            "create",
            "--title",
            "Linker",
            "--body",
            &format!("See [[{a_id}]]."),
        ])
        .assert()
        .success();

    repo.zdb().arg("reindex").assert().success();

    // Target should NOT be an orphan (it has an incoming link)
    repo.zdb()
        .args(["discover", "orphans"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&a_id).not());
}
