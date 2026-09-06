use crate::common::{stdout, DdbTestRepo};
use predicates::prelude::*;

#[test]
fn smoke_22_fts5_search_boost() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["type", "install", "contact"])
        .assert()
        .success();
    let id = stdout(
        &repo,
        &[
            "create",
            "--type",
            "contact",
            "--title",
            "Boost Test Contact",
            "--set",
            "email=uniquexyz@example.com",
        ],
    );
    repo.ddb().arg("reindex").assert().success();

    repo.ddb()
        .args(["search", "uniquexyz"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id));
}

#[test]
fn smoke_26_fts_negation() {
    let repo = DdbTestRepo::init();
    let id1 = stdout(
        &repo,
        &[
            "create",
            "--title",
            "Important Design",
            "--body",
            "important design review",
        ],
    );
    let id2 = stdout(
        &repo,
        &[
            "create",
            "--title",
            "Important Meeting",
            "--body",
            "important meeting notes",
        ],
    );
    let id3 = stdout(
        &repo,
        &[
            "create",
            "--title",
            "Daily Standup",
            "--body",
            "daily standup agenda",
        ],
    );
    repo.ddb().arg("reindex").assert().success();

    repo.ddb()
        .args(["search", "important NOT meeting"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id1))
        .stdout(predicate::str::contains(&id2).not());
    repo.ddb()
        .args(["search", "NOT standup"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id3).not())
        .stdout(predicate::str::contains(&id1));
}

#[test]
fn smoke_28_in_query_field_filter() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE smoke_cat (label VARCHAR(100))"])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "CREATE TABLE smoke_link (url TEXT, smoke_cat VARCHAR(14) REFERENCES smoke_cat(id))",
        ])
        .assert()
        .success();
    let category_id = stdout(
        &repo,
        &[
            "query",
            "INSERT INTO smoke_cat (title, label) VALUES ('Development', 'dev')",
        ],
    );
    repo.ddb()
        .args([
            "query",
            &format!(
                "INSERT INTO smoke_link (title, url, smoke_cat) VALUES ('Rust Async', 'https://example.com/rust-async', '{category_id}')"
            ),
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO smoke_link (title, url) VALUES ('Meeting Notes Archive', 'https://example.com/archive')",
        ])
        .assert()
        .success();
    repo.ddb().arg("reindex").assert().success();

    repo.ddb()
        .args(["search", "title=Archive"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Meeting Notes Archive"));
    repo.ddb()
        .args(["search", "smoke_cat=Development"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust Async"));
    repo.ddb()
        .args(["query", "DROP TABLE smoke_link CASCADE"])
        .assert()
        .success();
    repo.ddb()
        .args(["query", "DROP TABLE smoke_cat CASCADE"])
        .assert()
        .success();
}
