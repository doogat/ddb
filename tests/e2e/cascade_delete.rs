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
        .args(["query", "INSERT INTO category (label) VALUES ('tech')"])
        .output()
        .unwrap();
    assert!(
        cat_out.status.success(),
        "fixture command failed: {cat_out:?}"
    );
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
    assert!(
        bm_out.status.success(),
        "fixture command failed: {bm_out:?}"
    );
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
fn cascade_delete_cleans_owned_junction_when_parent_deleted() {
    // PRD 00137 §AC2: drives the parent-side cascade through the `ddb`
    // binary (CLI/SQL surface). Mirrors `cascade_delete_cleans_junction_table`
    // but deletes the bookmark (the junction's *owner*) rather than the
    // category (the *referenced target*). Pre-fix the junction row stayed
    // dangling; post-fix it is removed in the same transaction as the
    // typed-row delete.
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
        .args(["query", "INSERT INTO category (label) VALUES ('alpha')"])
        .output()
        .unwrap();
    assert!(
        cat_out.status.success(),
        "fixture command failed: {cat_out:?}"
    );
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Drive the auto-junction materializer end-to-end: setting the parent's
    // REFERENCES column (PRD 00134 path) creates the junction row. Pre-PRD
    // 00137 the junction stayed dangling on parent delete; post-fix this
    // round-trip cleans it.
    let bm_out = repo
        .ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO bookmark (url, category) VALUES ('https://example.com', '{cat_id}')"
            ),
        ])
        .output()
        .unwrap();
    assert!(
        bm_out.status.success(),
        "fixture command failed: {bm_out:?}"
    );
    let bm_id = String::from_utf8_lossy(&bm_out.stdout).trim().to_string();

    // Sanity: junction row exists keyed by the bookmark (parent) side.
    repo.ddb()
        .args([
            "query",
            &format!("SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1"));

    // Delete the bookmark (parent direction).
    repo.ddb()
        .args([
            "query",
            &format!("DELETE FROM bookmark WHERE id = '{bm_id}'"),
        ])
        .assert()
        .success();

    // Owner-side junction row must be gone.
    repo.ddb()
        .args([
            "query",
            &format!("SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("0"));

    // Stronger assertion: the only junction row was the deleted parent's,
    // so the junction table is now empty.
    repo.ddb()
        .args(["query", "SELECT COUNT(*) FROM bookmark_category"])
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
        .args(["query", "INSERT INTO category (label) VALUES ('tech')"])
        .output()
        .unwrap();
    assert!(
        cat_out.status.success(),
        "fixture command failed: {cat_out:?}"
    );
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
    assert!(
        bm_out.status.success(),
        "fixture command failed: {bm_out:?}"
    );
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
        .args(["query", "INSERT INTO category (label) VALUES ('tech')"])
        .output()
        .unwrap();
    assert!(
        cat_out.status.success(),
        "fixture command failed: {cat_out:?}"
    );
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
    assert!(
        bm_out.status.success(),
        "fixture command failed: {bm_out:?}"
    );
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
    repo.ddb().args(["delete", &cat_id]).assert().success();

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

// Issue #10: RESTRICT semantics for typed tables with NOT NULL REFERENCES.
// Both the SQL `DELETE FROM parent` path and the CLI `ddb delete` path must
// reject the delete when a child row would be left with NULL in a NOT NULL
// FK column.

#[test]
fn delete_rejected_by_not_null_references_sql_issue_10() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE link (url VARCHAR(255) NOT NULL)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE category (name VARCHAR(255) NOT NULL)",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE \"category-membership\" (\
                 link_id VARCHAR(255) NOT NULL REFERENCES link(id),\
                 category_id VARCHAR(255) NOT NULL REFERENCES category(id),\
                 UNIQUE(link_id, category_id)\
             )",
        ])
        .assert()
        .success();

    let link_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO link (title, url) VALUES ('L', 'https://a.com')",
        ])
        .output()
        .unwrap();
    assert!(
        link_out.status.success(),
        "fixture command failed: {link_out:?}"
    );
    let link_id = String::from_utf8_lossy(&link_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let cat_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO category (title, name) VALUES ('C', 'c')",
        ])
        .output()
        .unwrap();
    assert!(
        cat_out.status.success(),
        "fixture command failed: {cat_out:?}"
    );
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();

    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO \"category-membership\" (title, link_id, category_id) \
                 VALUES ('M', '{link_id}', '{cat_id}')"
            ),
        ])
        .assert()
        .success();

    // SQL DELETE must fail and report the blocker.
    repo.ddb()
        .args(["query", &format!("DELETE FROM link WHERE id = '{link_id}'")])
        .assert()
        .failure()
        .stderr(predicate::str::contains("NOT NULL REFERENCES"))
        .stderr(predicate::str::contains("category-membership"))
        .stderr(predicate::str::contains("link_id"));

    // The link is still there and the child row still holds the FK.
    repo.ddb()
        .args([
            "query",
            &format!("SELECT COUNT(*) FROM link WHERE id = '{link_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1"));
    repo.ddb()
        .args([
            "query",
            &format!("SELECT COUNT(*) FROM \"category-membership\" WHERE link_id = '{link_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1"));

    // Removing the blocker restores the same parent's normal delete path.
    repo.ddb()
        .args([
            "query",
            &format!("DELETE FROM \"category-membership\" WHERE link_id = '{link_id}'"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));
    repo.ddb()
        .args(["query", &format!("DELETE FROM link WHERE id = '{link_id}'")])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 row(s) affected"));
}

#[test]
fn delete_rejected_by_not_null_references_cli_issue_10() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE link (url VARCHAR(255) NOT NULL)"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE category (name VARCHAR(255) NOT NULL)",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE \"category-membership\" (\
                 link_id VARCHAR(255) NOT NULL REFERENCES link(id),\
                 category_id VARCHAR(255) NOT NULL REFERENCES category(id),\
                 UNIQUE(link_id, category_id)\
             )",
        ])
        .assert()
        .success();

    let link_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO link (title, url) VALUES ('L', 'https://a.com')",
        ])
        .output()
        .unwrap();
    assert!(
        link_out.status.success(),
        "fixture command failed: {link_out:?}"
    );
    let link_id = String::from_utf8_lossy(&link_out.stdout).trim().to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let cat_out = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO category (title, name) VALUES ('C', 'c')",
        ])
        .output()
        .unwrap();
    assert!(
        cat_out.status.success(),
        "fixture command failed: {cat_out:?}"
    );
    let cat_id = String::from_utf8_lossy(&cat_out.stdout).trim().to_string();
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO \"category-membership\" (title, link_id, category_id) \
                 VALUES ('M', '{link_id}', '{cat_id}')"
            ),
        ])
        .assert()
        .success();

    // `ddb delete` must also reject the delete.
    repo.ddb()
        .args(["delete", &link_id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("NOT NULL REFERENCES"))
        .stderr(predicate::str::contains("category-membership.link_id"));
}
