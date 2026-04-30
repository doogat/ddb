//! Regression tests for in-query field-filter semantics in `search()`.
//!
//! PRD 00133 (option B). Two in-scope contracts pinned here:
//!
//! 1. `title=<value>` via in-query syntax does LIKE/substring match against
//!    `z.title`. The explicit `where:` API filter for `title` stays Eq.
//! 2. `<directly_REFERENCES_field>=<value>` via in-query syntax resolves
//!    `<value>` against the referenced typedef's `title` with LIKE, then
//!    matches rows whose REFERENCES column ID is in the resolved set.
//!
//! Junction-membership-typedef traversal (e.g. jink's `category=work.dev`
//! via a separate `category-membership` typedef whose `category_id`
//! REFERENCES category and whose `link_id` REFERENCES link) is OUT OF
//! SCOPE for this PRD and is not covered here.
//!
//! All three tests below MUST currently FAIL on master — they are the
//! bisect harness and the permanent regression coverage. After the fix in
//! Task #3 they MUST pass.
use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use ddb_core::sql_engine::{SqlEngine, SqlResult};
use tempfile::TempDir;

fn setup() -> (TempDir, GitRepo, Index) {
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index = Index::open(&db_path).unwrap();
    (dir, repo, index)
}

fn exec_id(repo: &GitRepo, index: &Index, sql: &str) -> String {
    let mut engine = SqlEngine::new(index, repo);
    match engine.execute(sql).unwrap() {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?} for sql: {sql}"),
    }
}

fn exec_ok(repo: &GitRepo, index: &Index, sql: &str) {
    let mut engine = SqlEngine::new(index, repo);
    engine.execute(sql).unwrap();
}

fn add_tag(repo: &GitRepo, index: &Index, doogat_id: &str, tag: &str) {
    // Tags travel through frontmatter, not SQL columns. Read the doogat,
    // append the tag, write it back via the repo, and reindex.
    let path = index.resolve_path(doogat_id).unwrap();
    let original = repo.read_file(&path).unwrap();
    // Naive frontmatter rewrite — the freshly created doogat has no
    // tags line. Insert one before the `type:` line (always present on
    // typed doogats).
    let with_tag = if let Some(idx) = original.find("\ntype:") {
        let (head, tail) = original.split_at(idx + 1);
        format!("{head}tags: [{tag}]\n{tail}")
    } else {
        panic!("doogat at {path} has no `type:` line; cannot insert tags");
    };
    repo.commit_file(&path, &with_tag, "test: add tag").unwrap();
    let parsed = ddb_core::parser::parse(&with_tag, &path).unwrap();
    index.index_doogat(&parsed).unwrap();
}

/// Seed a fixture mirroring the in-scope half of jink's search fixture:
/// - a `category` typedef with title-bearing rows
/// - a `link` typedef with a direct REFERENCES column to `category`
/// - 3 link rows: 2 in `Development`, 1 in `Portals`, 1 with title containing `Archive`
fn seed(repo: &GitRepo, index: &Index) -> Vec<String> {
    exec_ok(
        repo,
        index,
        "CREATE TABLE category (label VARCHAR(100))",
    );
    exec_ok(
        repo,
        index,
        "CREATE TABLE link (url TEXT, category VARCHAR(14) REFERENCES category(id))",
    );

    let dev_id = exec_id(
        repo,
        index,
        "INSERT INTO category (title, label) VALUES ('Development', 'dev')",
    );
    let portals_id = exec_id(
        repo,
        index,
        "INSERT INTO category (title, label) VALUES ('Portals', 'portals')",
    );

    let l1 = exec_id(
        repo,
        index,
        &format!(
            "INSERT INTO link (title, url, category) VALUES ('Rust Async', 'https://example.com/rust-async', '{dev_id}')"
        ),
    );
    let l2 = exec_id(
        repo,
        index,
        &format!(
            "INSERT INTO link (title, url, category) VALUES ('Rust Errors', 'https://example.com/rust-errors', '{dev_id}')"
        ),
    );
    let l3 = exec_id(
        repo,
        index,
        &format!(
            "INSERT INTO link (title, url, category) VALUES ('Svelte Guide', 'https://example.com/svelte', '{portals_id}')"
        ),
    );
    let l4 = exec_id(
        repo,
        index,
        "INSERT INTO link (title, url) VALUES ('Meeting Notes Archive', 'https://example.com/archive')",
    );

    add_tag(repo, index, &l1, "rust");
    add_tag(repo, index, &l2, "rust");

    vec![l1, l2, l3, l4]
}

/// title= via in-query syntax must do LIKE/substring match.
/// Currently FAILS on master: build_filter_clauses routes `title` through
/// the core-column branch as `z.title = ?` (exact), so 'Archive' does
/// not match 'Meeting Notes Archive'.
#[test]
fn search_title_field_filter_does_substring_match() {
    let (_dir, repo, index) = setup();
    let _ids = seed(&repo, &index);

    let hits = index
        .search("title=Archive")
        .expect("search title=Archive should not error");

    let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
    assert!(
        titles.iter().any(|t| t.contains("Archive")),
        "expected at least one hit whose title contains 'Archive'; got titles: {titles:?}",
    );
    assert!(
        !hits.is_empty(),
        "expected at least 1 hit for title=Archive; got 0",
    );
}

/// Direct REFERENCES field-filter resolves `value` against the referenced
/// typedef's `title` with LIKE.
/// Currently FAILS on master: build_filter_clauses materialized-table
/// branch does `WHERE "category" = ?` directly against the materialized
/// column, which stores the referenced doogat's ID (a 14-digit
/// timestamp). 'Development' is the category's TITLE, not its ID, so 0
/// rows match.
#[test]
fn search_category_field_filter_resolves_via_referenced_title() {
    let (_dir, repo, index) = setup();
    let _ids = seed(&repo, &index);

    let hits = index
        .search("category=Development")
        .expect("search category=Development should not error");

    let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(
        hits.len(),
        2,
        "expected 2 hits for category=Development (Rust Async, Rust Errors); got titles: {titles:?}",
    );
    assert!(
        titles.contains(&"Rust Async"),
        "missing 'Rust Async': {titles:?}",
    );
    assert!(
        titles.contains(&"Rust Errors"),
        "missing 'Rust Errors': {titles:?}",
    );
}

/// Tag + REFERENCES intersection. Currently FAILS on master for the same
/// reason as `search_category_field_filter_resolves_via_referenced_title`:
/// the category clause filters out everything.
#[test]
fn search_tag_and_category_intersection_returns_intersection() {
    let (_dir, repo, index) = setup();
    let _ids = seed(&repo, &index);

    let hits = index
        .search("tag=rust AND category=Development")
        .expect("search should not error");

    let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(
        hits.len(),
        2,
        "expected 2 hits for tag=rust AND category=Development; got titles: {titles:?}",
    );
    assert!(
        titles.contains(&"Rust Async"),
        "missing 'Rust Async': {titles:?}",
    );
    assert!(
        titles.contains(&"Rust Errors"),
        "missing 'Rust Errors': {titles:?}",
    );
}
