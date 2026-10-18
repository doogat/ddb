//! Tests for PRD 00176 cycle-1 rework task 7 review findings: GraphQL
//! `search` / `doogats` default-limit pagination, and warning forwarding on
//! REST search and the GraphQL `search` `where`-filter branch.
//!
//! Written from requirements only, before any implementation fix for these
//! findings. See `docs/dev/tmp/dispatch-tess-7.txt` findings 1, 2, 3/4, 5, 6.

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// A far-future id so the poison file never collides with a minted one.
const POISON_PATH: &str = "ddb/29990101000000.md";

/// Run a raw `git` command in `repo`, asserting it succeeded. (Mirrors the
/// helper in `poison_file_reindex.rs`: each e2e test file is a separate
/// module, so it needs its own copy rather than a shared import.)
fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git failed to run");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Commit a doogat file with unparsable YAML frontmatter directly via raw
/// git, bypassing `ddb`'s own validation (the same delivery path sync/hand
/// edits use). A reindex over this repo must skip the file and report a
/// warning rather than fail the whole batch.
fn commit_poison_file(repo: &DdbTestRepo) {
    std::fs::write(
        repo.path().join(POISON_PATH),
        "---\n: invalid yaml [\n---\nbody\n",
    )
    .expect("failed to write poison file");
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-m", "add poison doogat"]);
}

fn assert_graphql_ok(result: &Value) {
    assert!(
        result.get("errors").is_none(),
        "GraphQL request failed: {result}"
    );
    assert!(
        result.get("data").is_some(),
        "missing GraphQL data: {result}"
    );
}

fn create_doogat(server: &ServerGuard, input: Value) -> Value {
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        json!({ "input": input }),
    );
    assert_graphql_ok(&result);
    result["data"]["createDoogat"].clone()
}

/// Seed `count` doogats whose titles all contain `term`, so a search for
/// `term` matches every one of them.
fn seed_matching(server: &ServerGuard, term: &str, count: usize) {
    for i in 0..count {
        create_doogat(server, json!({ "title": format!("{term} {i}") }));
    }
}

/// Finding 1 (HIGH): with no `limit` arg and more than 50 matches, `search`
/// must return exactly 50 hits (the new default limit), not every match.
#[test]
fn graphql_search_without_limit_defaults_to_50_hits() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    seed_matching(&server, "Pag1term", 55);

    let result = server.graphql(r#"{ search(query: "Pag1term") { totalCount hits { id } } }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 55, "{result}");
    let hits = result["data"]["search"]["hits"]
        .as_array()
        .expect("search missing hits");
    assert_eq!(
        hits.len(),
        50,
        "search with no limit must default to 50 hits, got {}: {result}",
        hits.len()
    );
}

/// Finding 2 (HIGH), part 1: `doogats` with no `limit` arg must be bounded
/// by the same default (50) as `search`, not return every doogat.
#[test]
fn graphql_doogats_without_limit_defaults_to_50() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    seed_matching(&server, "Pag2doogat", 55);

    let result = server.graphql(r#"{ doogats { id } }"#);
    assert_graphql_ok(&result);
    let items = result["data"]["doogats"]
        .as_array()
        .expect("doogats missing list");
    assert_eq!(
        items.len(),
        50,
        "doogats with no limit must default to 50, got {}: {result}",
        items.len()
    );
}

/// Finding 2 (HIGH), part 2: `limit: 0` on `doogats` must be rejected as
/// invalid, not silently accepted as "no limit".
#[test]
fn graphql_doogats_rejects_zero_limit() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    create_doogat(&server, json!({ "title": "Pag2zero" }));

    let result = server.graphql(r#"{ doogats(limit: 0) { id } }"#);
    let errors = result["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("limit: 0 must be rejected, not accepted: {result}"));
    assert!(!errors.is_empty(), "{result}");
}

/// Finding 3 (HIGH) and 4 (MEDIUM): REST search must forward warnings (e.g.
/// a reindex-skipped-files warning from a malformed doogat in the repo) to
/// the client, alongside the existing `data` / `total_count` fields.
#[test]
fn rest_search_forwards_reindex_warnings() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = server.rest_post(
        "/doogats",
        json!({ "title": "Pag3valid", "body": "content" }),
    );
    assert_eq!(resp.status(), 201, "{}", resp.text().unwrap_or_default());
    commit_poison_file(&repo);

    let resp = server.rest_get("/doogats?q=Pag3valid");
    assert_eq!(
        resp.status(),
        200,
        "search must still succeed despite the poison file"
    );
    let body: Value = resp.json().expect("search returned invalid JSON");

    assert!(body.get("data").is_some(), "{body}");
    assert!(body.get("total_count").is_some(), "{body}");
    let warnings = body
        .get("warnings")
        .unwrap_or_else(|| panic!("REST search response missing `warnings` field: {body}"));
    assert!(
        warnings.is_array() && !warnings.as_array().unwrap().is_empty(),
        "REST search must forward the reindex-skipped warning: {body}"
    );
}

/// Finding 5 (MEDIUM): the GraphQL `search` resolver's `where`-filter branch
/// takes a different code path (`pool.search` instead of `SearchCommand`)
/// and must still forward reindex warnings, the same as the plain-query
/// branch exercised above.
#[test]
fn graphql_search_where_filter_forwards_reindex_warnings() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    create_doogat(
        &server,
        json!({ "title": "Pag5valid", "tags": ["pag5tag"] }),
    );
    commit_poison_file(&repo);

    let result = server.graphql(
        r#"{ search(query: "", where: [{field: "tag", eq: "pag5tag"}]) { hits { id } } }"#,
    );
    assert_graphql_ok(&result);
    let warnings = result["extensions"]["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("where-filter search must forward reindex warnings: {result}"));
    assert!(!warnings.is_empty(), "{result}");
}

/// Finding 6 (LOW): a REST search hit with no custom fields must still
/// serialize a `fields` key (e.g. empty object/null), matching FFI and
/// GraphQL, which always emit it. Today `fields` is omitted entirely via
/// `skip_serializing_if`.
#[test]
fn rest_search_hit_without_fields_still_has_fields_key() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let resp = server.rest_post(
        "/doogats",
        json!({ "title": "Pag6nofields", "body": "content" }),
    );
    assert_eq!(resp.status(), 201, "{}", resp.text().unwrap_or_default());

    let body: Value = server
        .rest_get("/doogats?q=Pag6nofields")
        .json()
        .expect("search returned invalid JSON");
    let hits = body["data"].as_array().expect("search missing data array");
    let hit = hits
        .iter()
        .find(|h| h["title"] == "Pag6nofields")
        .unwrap_or_else(|| panic!("search missing seeded hit: {body}"));
    assert!(
        hit.as_object().unwrap().contains_key("fields"),
        "REST search hit with no custom fields must still carry a `fields` key: {hit}"
    );
}
