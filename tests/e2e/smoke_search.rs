use crate::common::{stdout, DdbTestRepo};
use predicates::prelude::*;
use std::process::Command;

/// Run `ddb search` with `args` after the query; assert success and return stdout.
fn search_stdout(repo: &DdbTestRepo, args: &[&str]) -> String {
    let out = repo
        .ddb()
        .arg("search")
        .args(args)
        .output()
        .expect("ddb search failed to run");
    assert!(
        out.status.success(),
        "ddb search {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Hit titles in `ddb search` output: each hit starts with `[<id>] <title> (<path>)`.
fn hit_titles(output: &str) -> Vec<String> {
    let mut titles: Vec<String> = output
        .lines()
        .filter_map(|line| line.strip_prefix('['))
        .filter_map(|rest| rest.split_once("] "))
        .filter_map(|(_, rest)| rest.rsplit_once(" (").map(|(title, _)| title.to_string()))
        .collect();
    titles.sort_unstable();
    titles
}

/// PRD 00176: `ddb search --type/--tag` decode into `SearchCommand` filters.
/// Pre-00176 the CLI had no filter flags. Seeds two registered types plus one
/// untyped doogat with overlapping tags, all sharing one term.
#[test]
fn smoke_search_type_and_tag_flags_filter_hits() {
    let repo = DdbTestRepo::init();
    for sql in [
        "CREATE TABLE sf_recipe (cuisine VARCHAR(100))",
        "CREATE TABLE sf_dessert (cuisine VARCHAR(100))",
    ] {
        repo.ddb().args(["query", sql]).assert().success();
    }
    for args in [
        ["--title", "Thai soup", "--type", "sf_recipe", "--tags", "dinner", "--set", "cuisine=thai", "--body", "sfterm soup"].as_slice(),
        ["--title", "Lime cake", "--type", "sf_dessert", "--tags", "dinner,sweet", "--set", "cuisine=thai", "--body", "sfterm cake"].as_slice(),
        ["--title", "Plain note", "--tags", "sweet", "--body", "sfterm note"].as_slice(),
    ] {
        repo.ddb().arg("create").args(args).assert().success();
    }

    // Control: unfiltered sees all three, so the filters below narrow.
    assert_eq!(
        hit_titles(&search_stdout(&repo, &["sfterm"])),
        ["Lime cake", "Plain note", "Thai soup"]
    );

    let out = search_stdout(&repo, &["sfterm", "--type", "sf_recipe"]);
    assert_eq!(hit_titles(&out), ["Thai soup"]);
    // The widened CLI hit prints type, tags, and typed fields.
    assert!(out.contains("type: sf_recipe"), "{out}");
    assert!(out.contains("tags: dinner"), "{out}");
    assert!(out.contains("cuisine: thai"), "{out}");

    assert_eq!(
        hit_titles(&search_stdout(&repo, &["sfterm", "--tag", "sweet"])),
        ["Lime cake", "Plain note"]
    );
    assert_eq!(
        hit_titles(&search_stdout(&repo, &["sfterm", "--type", "sf_dessert", "--tag", "dinner"])),
        ["Lime cake"]
    );

    // A filter matching nothing prints no hits, not the unfiltered set.
    let out = search_stdout(&repo, &["sfterm", "--type", "sf_recipe", "--tag", "sweet"]);
    assert!(hit_titles(&out).is_empty(), "{out}");
    assert!(out.contains("no results"), "{out}");
}

/// Commit `count` doogats sharing `term` straight through git (one commit, so
/// seeding does not pay the ~1 create/sec id-mint ceiling), then reindex.
fn seed_raw_doogats(repo: &DdbTestRepo, count: usize, term: &str) {
    for k in 0..count {
        let id = format!("202001010000{k:02}");
        std::fs::write(
            repo.path().join(format!("ddb/{id}.md")),
            format!("---\nid: {id}\ntitle: Bulk {k}\n---\n{term} body {k}\n"),
        )
        .expect("failed to write seed doogat");
    }
    for args in [["add", "-A"].as_slice(), ["commit", "-q", "-m", "seed bulk doogats"].as_slice()] {
        let out = Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .output()
            .expect("git failed to run");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    repo.ddb().arg("reindex").assert().success();
}

/// PRD 00176: omitting `--limit`/`--offset` reaches `PageRequest`'s 50/0
/// defaults. Pre-00176 clap hardcoded `--limit 20`, so this printed 1-20.
#[test]
fn smoke_search_without_limit_or_offset_uses_page_request_defaults() {
    let repo = DdbTestRepo::init();
    seed_raw_doogats(&repo, 55, "bulkpageterm");

    let out = search_stdout(&repo, &["bulkpageterm"]);
    assert!(
        out.starts_with("Showing 1-50 of 55 results"),
        "default window must be limit 50, offset 0: {out}"
    );
    assert_eq!(hit_titles(&out).len(), 50, "{out}");

    let out = search_stdout(&repo, &["bulkpageterm", "--offset", "50"]);
    assert!(out.starts_with("Showing 51-55 of 55 results"), "{out}");
    assert_eq!(hit_titles(&out).len(), 5, "{out}");
}

/// PRD 00176: an out-of-range `--limit` is a validation error, never a clamp.
#[test]
fn smoke_search_rejects_out_of_range_limit() {
    let repo = DdbTestRepo::init();
    for limit in ["0", "10001"] {
        repo.ddb()
            .args(["search", "anything", "--limit", limit])
            .assert()
            .failure()
            .stderr(predicate::str::contains("limit must be between 1 and 10000"));
    }
}

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
