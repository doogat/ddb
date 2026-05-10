use crate::common::DdbTestRepo;
use predicates::prelude::*;

/// PRD 00127: title_template interpolation resolves `{ref_col.field}` to the
/// referenced doogat's field value at write time.
#[test]
fn membership_title_resolves_from_referenced_doogats() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE link (url TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "CREATE TABLE category (fqn TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE membership (link TEXT REFERENCES link, category TEXT REFERENCES category)",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "ALTER TABLE membership SET TITLE TEMPLATE '{link.title} in {category.fqn}'",
        ])
        .assert()
        .success();

    let link_id = run_and_capture_id(
        &repo,
        "INSERT INTO link (title, url) VALUES ('My Link', 'https://x')",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    let cat_id = run_and_capture_id(
        &repo,
        "INSERT INTO category (title, fqn) VALUES ('Cat', 'A/B')",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mem_id = run_and_capture_id(
        &repo,
        &format!("INSERT INTO membership (link, category) VALUES ('{link_id}', '{cat_id}')"),
    );

    // Verify SELECT returns composed title
    repo.ddb()
        .args([
            "query",
            &format!("SELECT title FROM membership WHERE id = '{mem_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("My Link in A/B"));
}

#[test]
fn update_recomputes_membership_title() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE link (url TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE membership (link TEXT REFERENCES link)",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "ALTER TABLE membership SET TITLE TEMPLATE '{link.title}'",
        ])
        .assert()
        .success();

    let link_a = run_and_capture_id(
        &repo,
        "INSERT INTO link (title, url) VALUES ('Link A', 'https://a')",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    let link_b = run_and_capture_id(
        &repo,
        "INSERT INTO link (title, url) VALUES ('Link B', 'https://b')",
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mem_id = run_and_capture_id(
        &repo,
        &format!("INSERT INTO membership (link) VALUES ('{link_a}')"),
    );

    repo.ddb()
        .args([
            "query",
            &format!("UPDATE membership SET link = '{link_b}' WHERE id = '{mem_id}'"),
        ])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            &format!("SELECT title FROM membership WHERE id = '{mem_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Link B"))
        .stdout(predicate::str::contains("Link A").not());
}

#[test]
fn typedef_with_bad_dotted_path_is_rejected() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE link (url TEXT)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE membership (link TEXT REFERENCES link)",
        ])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "ALTER TABLE membership SET TITLE TEMPLATE '{link.does_not_exist}'",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist on link"));
}

fn run_and_capture_id(repo: &DdbTestRepo, sql: &str) -> String {
    let out = repo.ddb().args(["query", sql]).output().unwrap();
    assert!(
        out.status.success(),
        "ddb query failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
