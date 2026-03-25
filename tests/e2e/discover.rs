use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn discover_stale() {
    let repo = DdbTestRepo::init();

    // Create a typedef via SQL DDL, which places it in ddb/_typedef/
    repo.ddb()
        .args(["query", "CREATE TABLE expiring (dummy TEXT)"])
        .assert()
        .success();

    // Find the typedef file and patch it to add stale_after_days
    let typedef_dir = repo.path().join("ddb/_typedef");
    let typedef_entry = std::fs::read_dir(&typedef_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
            content.contains("title: expiring")
        })
        .expect("typedef file not found");
    let typedef_path = typedef_entry.path();
    let content = std::fs::read_to_string(&typedef_path).unwrap();
    let patched = content.replace("type: _typedef", "type: _typedef\nstale_after_days: 1");
    std::fs::write(&typedef_path, &patched).unwrap();

    // Commit the change so git sees it
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["add", "-A"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["commit", "-m", "add stale_after_days"])
        .output()
        .unwrap();

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create a doogat of that type
    repo.ddb()
        .args([
            "create",
            "--title",
            "Expiring Note",
            "--type",
            "expiring",
            "--body",
            "Some content.",
        ])
        .assert()
        .success();

    repo.ddb().arg("reindex").assert().success();

    // The doogat was just committed, so its git revision date is "now".
    // With stale_after_days=1, it won't be stale yet.
    // Verify the command runs without error and reports no stale doogats.
    repo.ddb()
        .args(["discover", "stale"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no stale doogats"));
}

#[test]
fn discover_mentions() {
    let repo = DdbTestRepo::init();

    // Create doogat A with a distinctive title
    let a_out = repo
        .ddb()
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

    // Create doogat B whose body mentions "Meeting Notes" but doesn't link to A
    let b_out = repo
        .ddb()
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
    repo.ddb().arg("reindex").assert().success();

    // discover mentions should find B as an unlinked mention of A
    repo.ddb()
        .args(["discover", "mentions", &a_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(&b_id));
}

#[test]
fn discover_mentions_excludes_linked() {
    let repo = DdbTestRepo::init();

    // Create doogat A
    let a_out = repo
        .ddb()
        .args(["create", "--title", "Meeting Notes", "--body", "Agenda."])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create doogat C that mentions "Meeting Notes" AND links to A
    repo.ddb()
        .args([
            "create",
            "--title",
            "Linked Follow-up",
            "--body",
            &format!("See Meeting Notes at [[{a_id}]]."),
        ])
        .assert()
        .success();

    repo.ddb().arg("reindex").assert().success();

    // C should NOT appear because it already links to A
    repo.ddb()
        .args(["discover", "mentions", &a_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("no unlinked mentions"));
}

#[test]
fn discover_similar() {
    let repo = DdbTestRepo::init();

    // Create two doogats with shared tags
    let a_out = repo
        .ddb()
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

    repo.ddb()
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

    repo.ddb().arg("reindex").assert().success();

    // discover similar should return at least one result (not "no suggestions")
    repo.ddb()
        .args(["discover", "similar", &a_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("no suggestions").not())
        .stdout(predicate::str::contains("Rust Advanced"));
}

#[test]
fn discover_orphans() {
    let repo = DdbTestRepo::init();

    // Create doogat with no incoming links
    let out = repo
        .ddb()
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

    repo.ddb().arg("reindex").assert().success();

    // discover orphans should find it
    repo.ddb()
        .args(["discover", "orphans"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id))
        .stdout(predicate::str::contains("Island Note"));
}

#[test]
fn discover_orphans_excludes_linked() {
    let repo = DdbTestRepo::init();

    // Create target doogat
    let a_out = repo
        .ddb()
        .args(["create", "--title", "Target", "--body", "I am linked."])
        .output()
        .unwrap();
    let a_id = String::from_utf8_lossy(&a_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Create doogat that links to target
    repo.ddb()
        .args([
            "create",
            "--title",
            "Linker",
            "--body",
            &format!("See [[{a_id}]]."),
        ])
        .assert()
        .success();

    repo.ddb().arg("reindex").assert().success();

    // Target should NOT be an orphan (it has an incoming link)
    repo.ddb()
        .args(["discover", "orphans"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&a_id).not());
}
