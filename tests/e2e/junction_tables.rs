use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn junction_table_round_trip() {
    let repo = DdbTestRepo::init();

    // Create tables: bookmark with REFERENCES column, and the referenced category table
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("table bookmark created"));

    repo.ddb()
        .args(["query", "CREATE TABLE category (label VARCHAR(100))"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table category created"));

    // Insert a category
    let cat_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO category (label) VALUES ('tech')",
        ])
        .output()
        .unwrap();
    assert!(
        cat_out.status.success(),
        "category insert failed: {}",
        String::from_utf8_lossy(&cat_out.stderr)
    );
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();
    assert!(!cat_id.is_empty(), "category ID should not be empty");

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Insert a bookmark
    let bm_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO bookmark (url) VALUES ('https://example.com')",
        ])
        .output()
        .unwrap();
    assert!(
        bm_out.status.success(),
        "bookmark insert failed: {}",
        String::from_utf8_lossy(&bm_out.stderr)
    );
    let bm_id = String::from_utf8_lossy(&bm_out.stdout).trim().to_string();
    assert!(!bm_id.is_empty(), "bookmark ID should not be empty");

    // Insert into junction table
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ),
        ])
        .assert()
        .success();

    // Verify junction row exists
    repo.ddb()
        .args([
            "query",
            &format!("SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&cat_id));

    // Delete junction row
    repo.ddb()
        .args([
            "query",
            &format!(
                "DELETE FROM bookmark_category WHERE bookmark_id = '{bm_id}' AND category_id = '{cat_id}'"
            ),
        ])
        .assert()
        .success();

    // Verify junction table is empty
    repo.ddb()
        .args([
            "query",
            "SELECT COUNT(*) FROM bookmark_category",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0"));

    // DROP TABLE CASCADE should remove junction table too
    // Re-insert a junction row first so there's data to cascade
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ),
        ])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "DROP TABLE bookmark CASCADE"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dropped"));

    // Junction table should no longer exist
    repo.ddb()
        .args(["query", "SELECT * FROM bookmark_category"])
        .assert()
        .failure();
}

#[test]
fn junction_table_survives_reindex() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE article (title TEXT, tag TEXT REFERENCES tag)",
        ])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "CREATE TABLE tag (name VARCHAR(50))"])
        .assert()
        .success();

    let tag_out = repo
        .ddb()
        .args(["query", "INSERT INTO tag (name) VALUES ('rust')"])
        .output()
        .unwrap();
    let tag_id = String::from_utf8_lossy(&tag_out.stdout).trim().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let art_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO article (title) VALUES ('Rust Guide')",
        ])
        .output()
        .unwrap();
    let art_id = String::from_utf8_lossy(&art_out.stdout).trim().to_string();

    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO article_tag (article_id, tag_id) VALUES ('{art_id}', '{tag_id}')"
            ),
        ])
        .assert()
        .success();

    // Reindex
    repo.ddb().arg("reindex").assert().success();

    // Junction data should survive reindex
    repo.ddb()
        .args([
            "query",
            &format!("SELECT tag_id FROM article_tag WHERE article_id = '{art_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&tag_id));
}

#[test]
fn junction_table_multiple_references() {
    // A type with two REFERENCES columns gets two junction tables
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE task (title TEXT, assignee TEXT REFERENCES person, project TEXT REFERENCES project)",
        ])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "CREATE TABLE person (name VARCHAR(100))"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "CREATE TABLE project (name VARCHAR(100))"])
        .assert()
        .success();

    let person_out = repo
        .ddb()
        .args(["query", "INSERT INTO person (name) VALUES ('alice')"])
        .output()
        .unwrap();
    let person_id = String::from_utf8_lossy(&person_out.stdout)
        .trim()
        .to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let proj_out = repo
        .ddb()
        .args(["query", "INSERT INTO project (name) VALUES ('alpha')"])
        .output()
        .unwrap();
    let proj_id = String::from_utf8_lossy(&proj_out.stdout)
        .trim()
        .to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let task_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO task (title) VALUES ('implement feature')",
        ])
        .output()
        .unwrap();
    let task_id = String::from_utf8_lossy(&task_out.stdout)
        .trim()
        .to_string();

    // Insert into both junction tables
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO task_assignee (task_id, assignee_id) VALUES ('{task_id}', '{person_id}')"
            ),
        ])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO task_project (task_id, project_id) VALUES ('{task_id}', '{proj_id}')"
            ),
        ])
        .assert()
        .success();

    // Verify both junction tables
    repo.ddb()
        .args([
            "query",
            &format!("SELECT assignee_id FROM task_assignee WHERE task_id = '{task_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&person_id));

    repo.ddb()
        .args([
            "query",
            &format!("SELECT project_id FROM task_project WHERE task_id = '{task_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&proj_id));
}
