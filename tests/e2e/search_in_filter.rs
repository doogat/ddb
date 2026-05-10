use crate::common::{DdbTestRepo, ServerGuard};
use std::time::Duration;

#[test]
fn search_where_in_tag() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Rust Note", "content": "infilter rust content", "tags": ["rust"] } }),
    );
    assert!(r.get("errors").is_none(), "create rust failed: {r}");
    std::thread::sleep(Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Svelte Note", "content": "infilter svelte content", "tags": ["svelte"] } }),
    );
    assert!(r.get("errors").is_none(), "create svelte failed: {r}");
    std::thread::sleep(Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Python Note", "content": "infilter python content", "tags": ["python"] } }),
    );
    assert!(r.get("errors").is_none(), "create python failed: {r}");

    // in: ["rust", "svelte"] should match 2 of 3
    let result = server.graphql(
        r#"{ search(query: "infilter", where: [{field: "tag", in: ["rust", "svelte"]}]) { hits { id title } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "search where in tag failed: {result}"
    );
    let hits = result["data"]["search"]["hits"].as_array().unwrap();
    assert_eq!(
        hits.len(),
        2,
        "expected 2 hits for tag in [rust, svelte], got: {hits:?}"
    );
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 2);

    let titles: Vec<&str> = hits.iter().map(|h| h["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Rust Note"),
        "missing Rust Note in {titles:?}"
    );
    assert!(
        titles.contains(&"Svelte Note"),
        "missing Svelte Note in {titles:?}"
    );
}

#[test]
fn search_where_in_materialized_column() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a type with a status column
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE task (status TEXT NOT NULL)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE failed: {r}");

    // Insert rows with different statuses
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO task (title, status) VALUES ('Task Open', 'open')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT open failed: {r}");
    std::thread::sleep(Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO task (title, status) VALUES ('Task Done', 'done')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT done failed: {r}");
    std::thread::sleep(Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO task (title, status) VALUES ('Task Blocked', 'blocked')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT blocked failed: {r}");

    // in: ["open", "done"] should match 2 of 3
    let result = server.graphql(
        r#"{ search(query: "Task", where: [{field: "status", in: ["open", "done"]}]) { hits { id title } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "search where in materialized failed: {result}"
    );
    let hits = result["data"]["search"]["hits"].as_array().unwrap();
    assert_eq!(
        hits.len(),
        2,
        "expected 2 hits for status in [open, done], got: {hits:?}"
    );
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 2);

    let titles: Vec<&str> = hits.iter().map(|h| h["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Task Open"),
        "missing Task Open in {titles:?}"
    );
    assert!(
        titles.contains(&"Task Done"),
        "missing Task Done in {titles:?}"
    );
}

#[test]
fn search_where_in_empty_returns_no_results() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Exists", "content": "emptyinfilter content", "tags": ["alive"] } }),
    );
    assert!(r.get("errors").is_none(), "create failed: {r}");

    // in: [] should match nothing
    let result = server.graphql(
        r#"{ search(query: "emptyinfilter", where: [{field: "tag", in: []}]) { hits { id title } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "search where in empty failed: {result}"
    );
    let hits = result["data"]["search"]["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 0, "expected 0 hits for in: [], got: {hits:?}");
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 0);
}
