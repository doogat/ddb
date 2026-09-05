use crate::common::DdbTestRepo;
use predicates::prelude::*;

#[test]
fn full_text_search() {
    let repo = DdbTestRepo::init();
    let created = repo
        .ddb()
        .args([
            "create",
            "--title",
            "Alpha",
            "--body",
            "uniquekeywordalpha here",
        ])
        .assert()
        .success();
    let id = String::from_utf8_lossy(&created.get_output().stdout)
        .trim()
        .to_owned();
    assert!(!id.is_empty(), "search fixture must return an id");
    std::thread::sleep(std::time::Duration::from_secs(1));
    repo.ddb()
        .args([
            "create",
            "--title",
            "Beta",
            "--body",
            "something else entirely",
        ])
        .assert()
        .success();

    repo.ddb()
        .args(["search", "uniquekeywordalpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alpha"))
        .stdout(predicate::str::contains("Beta").not());
    repo.ddb()
        .args(["search", "Alpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id));
}

#[test]
fn wikilink_indexed_in_links_table() {
    let repo = DdbTestRepo::init();
    let parent_out = repo
        .ddb()
        .args([
            "create",
            "--title",
            "Parent Note",
            "--body",
            "I am the parent.",
        ])
        .output()
        .unwrap();
    assert!(
        parent_out.status.success(),
        "fixture command failed: {parent_out:?}"
    );
    let parent_id = String::from_utf8_lossy(&parent_out.stdout)
        .trim()
        .to_string();
    std::thread::sleep(std::time::Duration::from_secs(1));

    repo.ddb()
        .args([
            "create",
            "--title",
            "Child Note",
            "--body",
            &format!("Links to [[{parent_id}|Parent Note]]."),
        ])
        .assert()
        .success();

    repo.ddb()
        .args([
            "query",
            "SELECT source_id, target_path, display FROM _ddb_links",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(&parent_id))
        .stdout(predicate::str::contains("Parent Note"));
}

#[test]
fn tags_indexed() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "Tagged", "--tags", "rust,testing"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "SELECT tag FROM _ddb_tags ORDER BY tag"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rust"))
        .stdout(predicate::str::contains("testing"));
}

#[test]
fn doogats_table_queryable() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["create", "--title", "Queryable", "--body", "test"])
        .assert()
        .success();

    repo.ddb()
        .args(["query", "SELECT id, title FROM doogats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Queryable"));
}

#[test]
fn search_returns_snippet_with_highlight() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "create",
            "--title",
            "Highlight Test",
            "--body",
            "The searchterm appears here.",
        ])
        .assert()
        .success();

    repo.ddb()
        .args(["search", "searchterm"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<b>searchterm</b>"));
}

#[test]
fn paginated_search_shows_header() {
    let repo = DdbTestRepo::init();
    for i in 0..5 {
        repo.ddb()
            .args([
                "create",
                "--title",
                &format!("Page {i}"),
                "--body",
                "paginatedsearchword here",
            ])
            .assert()
            .success();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    repo.ddb()
        .args([
            "search",
            "paginatedsearchword",
            "--limit",
            "2",
            "--offset",
            "0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Showing 1-2 of 5 results"));
}

#[test]
fn paginated_search_offset_beyond() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args([
            "create",
            "--title",
            "Solo",
            "--body",
            "offsetbeyondword here",
        ])
        .assert()
        .success();

    repo.ddb()
        .args([
            "search",
            "offsetbeyondword",
            "--limit",
            "10",
            "--offset",
            "100",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("no results"));
}
