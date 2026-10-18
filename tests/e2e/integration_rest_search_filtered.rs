//! REST search through the app contract (PRD 00176).
//!
//! `GET /rest/doogats?q=...` decodes into `SearchCommand` and `PageRequest`.
//! Before 00176 the search branch dropped `type` / `tag` (it passed
//! `SearchFilters::default()`), returned a four-field hit, and clamped
//! `per_page` silently to 1-200. These scenarios pin the fixed contract:
//! filters narrow the hits, every hit carries all ten core `SearchResult`
//! fields, and out-of-range paging is a 400 `VALIDATION_ERROR`.

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::{json, Value};

const TERM: &str = "rsfilterterm";

fn execute_sql(server: &ServerGuard, sql: &str) {
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        json!({ "sql": sql }),
    );
    assert!(result.get("errors").is_none(), "{sql} failed: {result}");
}

fn rest_create(server: &ServerGuard, body: Value) {
    let resp = server.rest_post("/doogats", body.clone());
    assert_eq!(resp.status(), 201, "REST create failed for {body}");
}

/// Two registered types plus one untyped doogat, with overlapping tags, all
/// sharing `TERM`:
/// - "Thai soup": `rsf_recipe`, tags [dinner], cuisine=thai
/// - "Lime cake": `rsf_dessert`, tags [dinner, sweet], cuisine=thai
/// - "Plain note": untyped, tags [sweet]
fn seed(server: &ServerGuard) {
    execute_sql(server, "CREATE TABLE rsf_recipe (cuisine VARCHAR(100))");
    execute_sql(server, "CREATE TABLE rsf_dessert (cuisine VARCHAR(100))");
    rest_create(
        server,
        json!({
            "title": "Thai soup", "body": format!("{TERM} soup"), "type": "rsf_recipe",
            "tags": ["dinner"], "fields": "{\"cuisine\":\"thai\"}",
        }),
    );
    rest_create(
        server,
        json!({
            "title": "Lime cake", "body": format!("{TERM} cake"), "type": "rsf_dessert",
            "tags": ["dinner", "sweet"], "fields": "{\"cuisine\":\"thai\"}",
        }),
    );
    rest_create(
        server,
        json!({ "title": "Plain note", "body": format!("{TERM} note"), "tags": ["sweet"] }),
    );
}

/// Run a REST search with extra query-string params; assert 200 and return the body.
fn search(server: &ServerGuard, extra: &str) -> Value {
    let resp = server.rest_get(&format!("/doogats?q={TERM}{extra}"));
    assert_eq!(resp.status(), 200, "search q={TERM}{extra} should return 200");
    resp.json().expect("search returned invalid JSON")
}

/// Sorted hit titles plus `total_count`.
fn titles_and_total(body: &Value) -> (Vec<String>, u64) {
    let mut titles: Vec<String> = body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("search body missing data array: {body}"))
        .iter()
        .map(|h| h["title"].as_str().expect("hit missing title").to_string())
        .collect();
    titles.sort_unstable();
    let total = body["total_count"]
        .as_u64()
        .unwrap_or_else(|| panic!("search body missing total_count: {body}"));
    (titles, total)
}

/// Assert `hit` carries all ten core `SearchResult` fields, with a typed hit's
/// type / tags / typed fields set to the seeded values.
fn assert_full_typed_hit(hit: &Value, doogat_type: &str, tags: &[&str]) {
    for key in ["id", "title", "path", "snippet", "updated_at"] {
        assert!(hit[key].is_string(), "hit field `{key}` must be a string: {hit}");
    }
    assert!(hit["rank"].is_number(), "hit field `rank` must be a number: {hit}");
    assert!(
        hit["path"].as_str().unwrap().ends_with(".md"),
        "hit path must name the doogat file: {hit}"
    );
    assert!(
        hit["created_at"].is_string(),
        "hit field `created_at` must be set: {hit}"
    );
    assert_eq!(hit["type"], doogat_type, "hit type: {hit}");
    assert_eq!(hit["tags"], json!(tags), "hit tags: {hit}");
    assert_eq!(hit["fields"], json!({ "cuisine": "thai" }), "hit typed fields: {hit}");
}

/// PRD 00176 Phase 1 acceptance: a REST search with type/tag filters returns
/// only the matching hits, each carrying tags/type/fields/created_at. Pre-00176
/// the filters were dropped, so every filtered request below returned all
/// three doogats with four fields each.
#[test]
fn rest_search_applies_type_and_tag_filters_and_returns_full_hit_shape() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    seed(&server);

    // Control: unfiltered search sees all three, so the filters below narrow.
    let (titles, total) = titles_and_total(&search(&server, ""));
    assert_eq!(titles, ["Lime cake", "Plain note", "Thai soup"]);
    assert_eq!(total, 3);

    let body = search(&server, "&type=rsf_recipe");
    assert_eq!(titles_and_total(&body), (vec!["Thai soup".to_string()], 1));
    assert_full_typed_hit(&body["data"][0], "rsf_recipe", &["dinner"]);

    let (titles, total) = titles_and_total(&search(&server, "&tag=sweet"));
    assert_eq!(titles, ["Lime cake", "Plain note"]);
    assert_eq!(total, 2);

    let body = search(&server, "&type=rsf_dessert&tag=dinner");
    assert_eq!(titles_and_total(&body), (vec!["Lime cake".to_string()], 1));
    assert_full_typed_hit(&body["data"][0], "rsf_dessert", &["dinner", "sweet"]);

    // Untyped hit: same field set, with type null and `fields` left out.
    let body = search(&server, "&tag=sweet");
    let plain = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["title"] == "Plain note")
        .unwrap_or_else(|| panic!("tag=sweet must include Plain note: {body}"));
    assert!(plain["type"].is_null(), "untyped hit type must be null: {plain}");
    assert_eq!(
        plain["fields"],
        json!({}),
        "untyped hit must carry an empty `fields` key, not omit it: {plain}"
    );
    assert_eq!(plain["tags"], json!(["sweet"]));
    assert!(plain["created_at"].is_string() && plain["path"].is_string(), "{plain}");
}

/// PRD Critical Scenario (error case): a filter that matches nothing returns an
/// empty hit list, not the unfiltered result `SearchFilters::default()` gave.
#[test]
fn rest_search_filter_matching_nothing_returns_empty_list() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    seed(&server);

    for extra in ["&type=rsf_recipe&tag=sweet", "&type=no_such_type"] {
        let body = search(&server, extra);
        assert_eq!(
            titles_and_total(&body),
            (Vec::<String>::new(), 0),
            "q={TERM}{extra} must return no hits: {body}"
        );
    }
}

/// REST search paging decodes through `PageRequest`: out-of-range values are a
/// 400 `VALIDATION_ERROR` instead of a silent clamp, and the ceiling is 10,000
/// (was 200). Pre-00176 every request below returned 200.
#[test]
fn rest_search_rejects_out_of_range_paging_instead_of_clamping() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    seed(&server);

    for extra in ["&per_page=10001", "&per_page=0", "&per_page=-5", "&page=0", "&page=-1"] {
        let resp = server.rest_get(&format!("/doogats?q={TERM}{extra}"));
        assert_eq!(resp.status(), 400, "q={TERM}{extra} must be rejected, not clamped");
        let body: Value = resp.json().expect("error body must be JSON");
        assert_eq!(body["error"], "VALIDATION_ERROR", "q={TERM}{extra}: {body}");
        assert!(body["message"].is_string(), "q={TERM}{extra}: {body}");
    }

    // Values the old 1-200 clamp would have cut down are now honored.
    for extra in ["&per_page=500", "&per_page=10000"] {
        assert_eq!(titles_and_total(&search(&server, extra)).1, 3, "q={TERM}{extra}");
    }

    // One-based page -> offset: page 2 of size 2 holds the third hit.
    let body = search(&server, "&page=2&per_page=2");
    assert_eq!(body["data"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["total_count"], 3, "{body}");

    // The plain list branch (no `q`) is out of 00176's scope and keeps its clamp.
    let resp = server.rest_get("/doogats?per_page=10001");
    assert_eq!(resp.status(), 200, "list branch must keep clamping");
    let body: Value = resp.json().expect("list returned invalid JSON");
    assert_eq!(body["pagination"]["per_page"], 200, "{body}");
}
