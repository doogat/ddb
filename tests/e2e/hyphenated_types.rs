use crate::common::{DdbTestRepo, ServerGuard};
use std::time::Duration;

#[test]
fn hyphenated_type_typed_query() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a typedef with hyphenated name
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": r#"CREATE TABLE "test-widget" (status TEXT, priority INTEGER)"# }),
    );
    assert!(r.get("errors").is_none(), "CREATE test-widget failed: {r}");

    // Insert a row
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": r#"INSERT INTO "test-widget" (status, priority) VALUES ('active', 1)"# }),
    );
    assert!(r.get("errors").is_none(), "INSERT test-widget failed: {r}");

    // Query via the typed query field (test-widget -> testWidgets)
    let r = server.graphql(r#"{ testWidgets { items { id title status priority } } }"#);
    assert!(
        r.get("errors").is_none(),
        "testWidgets query failed: {r}"
    );
    let items = r["data"]["testWidgets"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "expected 1 test-widget: {r}");
    assert_eq!(items[0]["status"].as_str().unwrap(), "active");
    assert_eq!(items[0]["priority"].as_i64().unwrap(), 1);
}

#[test]
fn hyphenated_type_coexists_with_normal() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a hyphenated type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": r#"CREATE TABLE "test-widget" (status TEXT)"# }),
    );
    assert!(r.get("errors").is_none(), "CREATE test-widget failed: {r}");

    // Create a normal type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE bookmark (url TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE bookmark failed: {r}");

    // Insert a row into test-widget
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": r#"INSERT INTO "test-widget" (status) VALUES ('draft')"# }),
    );
    assert!(r.get("errors").is_none(), "INSERT test-widget failed: {r}");

    std::thread::sleep(Duration::from_secs(1));

    // Insert a row into bookmark
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT bookmark failed: {r}");

    // Query both typed queries
    let r = server.graphql(r#"{ testWidgets { items { id } } bookmarks { items { id } } }"#);
    assert!(
        r.get("errors").is_none(),
        "combined query failed: {r}"
    );
    let widgets = r["data"]["testWidgets"]["items"].as_array().unwrap();
    assert_eq!(widgets.len(), 1, "expected 1 test-widget: {r}");
    let bookmarks = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1, "expected 1 bookmark: {r}");
}
