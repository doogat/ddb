use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn tag_count_group_by() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "A", "--tags", "rust,testing"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));
    repo.ddb()
        .args(["create", "--title", "B", "--tags", "rust,cli"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));
    repo.ddb()
        .args(["create", "--title", "C", "--tags", "cli"])
        .assert()
        .success();

    // rust appears 2 times, cli appears 2 times, testing appears 1 time
    repo.ddb()
        .args([
            "query",
            "SELECT tag, COUNT(*) as c FROM _ddb_tags GROUP BY tag ORDER BY c DESC, tag ASC",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("rust"))
        .stdout(predicate::str::contains("cli"))
        .stdout(predicate::str::contains("testing"));
}

#[test]
fn tag_join_filters_doogats() {
    let repo = DdbTestRepo::init();
    let created = repo
        .ddb()
        .args(["create", "--title", "HasRust", "--tags", "rust"])
        .assert()
        .success();
    let id = String::from_utf8_lossy(&created.get_output().stdout)
        .trim()
        .to_owned();
    assert!(!id.is_empty(), "tagged fixture must return an id");
    std::thread::sleep(std::time::Duration::from_secs(1));
    repo.ddb()
        .args(["create", "--title", "NoPython", "--tags", "python"])
        .assert()
        .success();

    // Join to find only doogats tagged "rust"
    repo.ddb()
        .args([
            "query",
            "SELECT z.title FROM doogats z JOIN _ddb_tags t ON t.doogat_id = z.id WHERE t.tag = 'rust'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("HasRust"))
        .stdout(predicate::str::contains("NoPython").not());

    repo.ddb()
        .args([
            "query",
            "SELECT z.id, z.title FROM doogats z JOIN _ddb_tags t ON t.doogat_id = z.id WHERE t.tag LIKE '%rust%'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id))
        .stdout(predicate::str::contains("HasRust"))
        .stdout(predicate::str::contains("NoPython").not());
}

#[test]
fn body_hashtag_indexed_with_source() {
    let repo = DdbTestRepo::init();
    let created = repo
        .ddb()
        .args([
            "create",
            "--title",
            "Hashtag Note",
            "--tags",
            "frontmatter-tag",
            "--body",
            "Some text with #body-tag inline.",
        ])
        .assert()
        .success();
    let id = String::from_utf8_lossy(&created.get_output().stdout)
        .trim()
        .to_owned();

    // Both frontmatter and body tags should be present with correct source
    repo.ddb()
        .args(["query", "SELECT tag, source FROM _ddb_tags ORDER BY tag"])
        .assert()
        .success()
        .stdout(predicate::str::contains("body-tag"))
        .stdout(predicate::str::contains("frontmatter-tag"));

    repo.ddb()
        .args([
            "update",
            &id,
            "--body",
            "Updated with #gtd/act/next hashtag",
        ])
        .assert()
        .success();
    repo.ddb().arg("reindex").assert().success();
    repo.ddb()
        .args([
            "query",
            "SELECT tag, source FROM _ddb_tags WHERE tag = 'gtd/act/next'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("gtd/act/next | body"));
}

#[test]
fn update_replaces_tags() {
    let repo = DdbTestRepo::init();
    let out = repo
        .ddb()
        .args(["create", "--title", "Mutable", "--tags", "old-tag"])
        .output()
        .unwrap();
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    repo.ddb()
        .args(["update", &id, "--tags", "new-tag"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "SELECT tag FROM _ddb_tags ORDER BY tag"])
        .assert()
        .success()
        .stdout(predicate::str::contains("new-tag"))
        .stdout(predicate::str::contains("old-tag").not());
}

#[test]
fn distinct_tags_across_doogats() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "D1", "--tags", "shared,unique1"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_secs(1));
    repo.ddb()
        .args(["create", "--title", "D2", "--tags", "shared,unique2"])
        .assert()
        .success();

    // DISTINCT tag should return 3 unique tags
    repo.ddb()
        .args(["query", "SELECT DISTINCT tag FROM _ddb_tags ORDER BY tag"])
        .assert()
        .success()
        .stdout(predicate::str::contains("shared"))
        .stdout(predicate::str::contains("unique1"))
        .stdout(predicate::str::contains("unique2"));
}
