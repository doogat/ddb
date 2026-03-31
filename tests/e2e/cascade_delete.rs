use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn cascade_delete_cleans_junction_table() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE category (label VARCHAR(100))"])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)",
        ])
        .assert()
        .success();

    let cat_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO category (label) VALUES ('tech')",
        ])
        .output()
        .unwrap();
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let bm_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO bookmark (url) VALUES ('https://example.com')",
        ])
        .output()
        .unwrap();
    let bm_id = String::from_utf8_lossy(&bm_out.stdout).trim().to_string();

    // Link bookmark -> category
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
            &format!("SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1"));

    // Delete the category — should cascade-clean junction table
    repo.ddb()
        .args([
            "query",
            &format!("DELETE FROM category WHERE id = '{cat_id}'"),
        ])
        .assert()
        .success();

    // Junction row should be gone
    repo.ddb()
        .args([
            "query",
            &format!("SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0"));
}

#[test]
fn cascade_delete_removes_wikilink_from_referencing_file() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE category (label VARCHAR(100))"])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)",
        ])
        .assert()
        .success();

    let cat_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO category (label) VALUES ('tech')",
        ])
        .output()
        .unwrap();
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let bm_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO bookmark (url) VALUES ('https://example.com')",
        ])
        .output()
        .unwrap();
    let bm_id = String::from_utf8_lossy(&bm_out.stdout).trim().to_string();

    // Link bookmark -> category (creates wikilink in reference section)
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ),
        ])
        .assert()
        .success();

    // Read bookmark file — should contain [[cat_id]]
    repo.ddb()
        .args(["get", &bm_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("[[{cat_id}]]")));

    // Delete the category
    repo.ddb()
        .args([
            "query",
            &format!("DELETE FROM category WHERE id = '{cat_id}'"),
        ])
        .assert()
        .success();

    // Bookmark file should no longer contain [[cat_id]]
    repo.ddb()
        .args(["get", &bm_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("[[{cat_id}]]")).not());
}

#[test]
fn cascade_delete_via_ddb_delete_cleans_junction_and_refs() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE category (label VARCHAR(100))"])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)",
        ])
        .assert()
        .success();

    let cat_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO category (label) VALUES ('tech')",
        ])
        .output()
        .unwrap();
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let bm_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO bookmark (url) VALUES ('https://example.com')",
        ])
        .output()
        .unwrap();
    let bm_id = String::from_utf8_lossy(&bm_out.stdout).trim().to_string();

    // Link bookmark -> category via junction
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ),
        ])
        .assert()
        .success();

    // Verify wikilink exists
    repo.ddb()
        .args(["get", &bm_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("[[{cat_id}]]")));

    // Delete via `ddb delete` (service path, not SQL engine)
    repo.ddb()
        .args(["delete", &cat_id])
        .assert()
        .success();

    // Junction row should be gone
    repo.ddb()
        .args([
            "query",
            &format!("SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0"));

    // Wikilink should be removed from bookmark file
    repo.ddb()
        .args(["get", &bm_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("[[{cat_id}]]")).not());
}
