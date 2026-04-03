use crate::common::{DdbTestRepo, ServerGuard};
use std::time::Duration;

/// Verify that a search where-filter on a REFERENCES field resolves via
/// the junction table join (eq operator).
#[test]
fn search_where_junction_eq() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create category type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE category (label TEXT NOT NULL)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE category failed: {r}");

    // Create link type with REFERENCES column
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE link (url TEXT, category TEXT REFERENCES category)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE link failed: {r}");

    // Insert a category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO category (label) VALUES ('tech')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT category failed: {r}");
    let cat_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_id.is_empty(), "category ID should not be empty");

    std::thread::sleep(Duration::from_secs(1));

    // Insert two links with unique searchable titles
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO link (title, url) VALUES ('juncteq Rust Portal', 'https://rust-lang.org')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT link1 failed: {r}");
    let link1_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();

    std::thread::sleep(Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO link (title, url) VALUES ('juncteq Example Site', 'https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT link2 failed: {r}");

    // Link only link1 to the category via junction table
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO link_category (link_id, category_id) VALUES ('{link1_id}', '{cat_id}')"
            )
        }),
    );
    assert!(
        r.get("errors").is_none(),
        "INSERT junction failed: {r}"
    );

    // Search with where filter on junction field (eq by category ID)
    let q = format!(
        r#"{{ search(query: "juncteq", where: [{{field: "category", eq: "{cat_id}"}}]) {{ hits {{ id title }} totalCount }} }}"#
    );
    let result = server.graphql(&q);
    assert!(
        result.get("errors").is_none(),
        "search where junction eq failed: {result}"
    );
    let hits = result["data"]["search"]["hits"].as_array().unwrap();
    assert_eq!(
        hits.len(),
        1,
        "expected 1 hit for category junction eq, got: {hits:?}"
    );
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 1);

    // Without the junction filter, both links should appear
    let result_all = server.graphql(
        r#"{ search(query: "juncteq") { hits { id } totalCount } }"#,
    );
    assert!(result_all.get("errors").is_none(), "unfiltered search failed: {result_all}");
    let all_hits = result_all["data"]["search"]["hits"].as_array().unwrap();
    assert_eq!(all_hits.len(), 2, "unfiltered search should return both links");
}

/// Verify that a junction table where-filter using the `contains` operator
/// matches by the referenced doogat's title (via JOIN on doogats).
#[test]
fn search_where_junction_contains() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create category type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE category (label TEXT NOT NULL)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE category failed: {r}");

    // Create link type with REFERENCES column
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE link (url TEXT, category TEXT REFERENCES category)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE link failed: {r}");

    // Insert a category with a recognisable title
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO category (title, label) VALUES ('Technology Hub', 'tech')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT category failed: {r}");
    let cat_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_id.is_empty(), "category ID should not be empty");

    std::thread::sleep(Duration::from_secs(1));

    // Insert two links with unique searchable prefix
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO link (title, url) VALUES ('junctcont Alpha', 'https://alpha.example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT link1 failed: {r}");
    let link1_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();

    std::thread::sleep(Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO link (title, url) VALUES ('junctcont Beta', 'https://beta.example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT link2 failed: {r}");

    // Link only link1 to the category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO link_category (link_id, category_id) VALUES ('{link1_id}', '{cat_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");

    // Search with contains on the category title substring
    let result = server.graphql(
        r#"{ search(query: "junctcont", where: [{field: "category", contains: "Technology"}]) { hits { id title } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "search where junction contains failed: {result}"
    );
    let hits = result["data"]["search"]["hits"].as_array().unwrap();
    assert_eq!(
        hits.len(),
        1,
        "expected 1 hit for category contains 'Technology', got: {hits:?}"
    );
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 1);

    // Without the filter both links should appear
    let result_all = server.graphql(r#"{ search(query: "junctcont") { hits { id } totalCount } }"#);
    assert!(result_all.get("errors").is_none(), "unfiltered search failed: {result_all}");
    let all_hits = result_all["data"]["search"]["hits"].as_array().unwrap();
    assert_eq!(all_hits.len(), 2, "unfiltered search should return both links");
}
