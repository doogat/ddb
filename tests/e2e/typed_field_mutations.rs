use crate::common::{DdbTestRepo, ServerGuard};

/// Helper: execute SQL via GraphQL and assert success.
fn exec_sql(server: &ServerGuard, sql: &str) -> serde_json::Value {
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": sql }),
    );
    assert!(r.get("errors").is_none(), "SQL failed: {r}\nSQL: {sql}");
    r
}

/// Helper: SELECT via executeSql with format:"objects", returns parsed row objects.
fn select_objects(server: &ServerGuard, sql: &str) -> Vec<serde_json::Value> {
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!, $fmt: String) { executeSql(sql: $sql, format: $fmt) { rows } }"#,
        serde_json::json!({ "sql": sql, "fmt": "objects" }),
    );
    assert!(r.get("errors").is_none(), "SELECT failed: {r}\nSQL: {sql}");
    r["data"]["executeSql"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| serde_json::from_str(row.as_str().unwrap()).unwrap())
        .collect()
}

#[test]
fn update_doogat_with_fields_updates_materialized_row() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create typedef with VARCHAR column (frontmatter zone)
    exec_sql(&server, "CREATE TABLE bookmark (url VARCHAR(200))");

    // Create a typed doogat via SQL INSERT
    let r = exec_sql(
        &server,
        "INSERT INTO bookmark (title, url) VALUES ('My Bookmark', 'https://old.com')",
    );
    let id = r["data"]["executeSql"]["message"].as_str().unwrap().trim();

    // Verify initial materialized row
    let rows = select_objects(
        &server,
        &format!("SELECT url FROM bookmark WHERE id = '{id}'"),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["url"].as_str().unwrap(), "https://old.com");

    // Update via GraphQL updateDoogat with fields
    let r = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id title } }"#,
        serde_json::json!({
            "input": {
                "id": id,
                "fields": "{\"url\":\"https://new.com\"}"
            }
        }),
    );
    assert!(
        r.get("errors").is_none(),
        "updateDoogat with fields failed: {r}"
    );
    assert_eq!(r["data"]["updateDoogat"]["id"].as_str().unwrap(), id);

    // Verify materialized row is updated
    let rows = select_objects(
        &server,
        &format!("SELECT url FROM bookmark WHERE id = '{id}'"),
    );
    assert_eq!(rows.len(), 1, "expected 1 row after update: {rows:?}");
    assert_eq!(
        rows[0]["url"].as_str().unwrap(),
        "https://new.com",
        "materialized url should be updated"
    );
}

#[test]
fn update_doogat_with_unset_fields_removes_field() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    exec_sql(&server, "CREATE TABLE bookmark (url VARCHAR(200))");

    let r = exec_sql(
        &server,
        "INSERT INTO bookmark (title, url) VALUES ('Unset Test', 'https://remove.me')",
    );
    let id = r["data"]["executeSql"]["message"].as_str().unwrap().trim();

    // Verify field is set
    let rows = select_objects(
        &server,
        &format!("SELECT url FROM bookmark WHERE id = '{id}'"),
    );
    assert_eq!(rows[0]["url"].as_str().unwrap(), "https://remove.me");

    // Unset the url field
    let r = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id } }"#,
        serde_json::json!({
            "input": {
                "id": id,
                "unsetFields": ["url"]
            }
        }),
    );
    assert!(
        r.get("errors").is_none(),
        "updateDoogat with unsetFields failed: {r}"
    );

    // Verify materialized row has NULL url
    let rows = select_objects(
        &server,
        &format!("SELECT url FROM bookmark WHERE id = '{id}'"),
    );
    assert_eq!(rows.len(), 1, "row should still exist: {rows:?}");
    assert!(
        rows[0]["url"].is_null() || rows[0]["url"].as_str().unwrap_or("") == "NULL",
        "url should be null/empty after unset, got: {:?}",
        rows[0]["url"]
    );
}

#[test]
fn batch_update_with_per_item_fields() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    exec_sql(&server, "CREATE TABLE bookmark (url VARCHAR(200))");

    let r1 = exec_sql(
        &server,
        "INSERT INTO bookmark (title, url) VALUES ('Batch A', 'https://a.com')",
    );
    let id1 = r1["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();

    let r2 = exec_sql(
        &server,
        "INSERT INTO bookmark (title, url) VALUES ('Batch B', 'https://b.com')",
    );
    let id2 = r2["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();

    // Batch update with per-item fields
    let query = format!(
        r#"mutation {{ batchUpdate(updates: [
            {{id: "{id1}", fields: "{{\"url\":\"https://a-updated.com\"}}"}},
            {{id: "{id2}", fields: "{{\"url\":\"https://b-updated.com\"}}"}}
        ]) {{ id title }} }}"#,
    );
    let r = server.graphql(&query);
    assert!(
        r.get("errors").is_none(),
        "batchUpdate with fields failed: {r}"
    );
    let updated = r["data"]["batchUpdate"].as_array().unwrap();
    assert_eq!(updated.len(), 2, "expected 2 results: {r}");

    // Verify materialized rows
    let rows = select_objects(
        &server,
        &format!("SELECT id, url FROM bookmark WHERE id IN ('{id1}', '{id2}') ORDER BY url"),
    );
    assert_eq!(rows.len(), 2, "expected 2 rows: {rows:?}");

    let urls: Vec<&str> = rows
        .iter()
        .map(|row| row["url"].as_str().unwrap())
        .collect();
    assert!(
        urls.contains(&"https://a-updated.com"),
        "expected a-updated.com in urls: {urls:?}"
    );
    assert!(
        urls.contains(&"https://b-updated.com"),
        "expected b-updated.com in urls: {urls:?}"
    );
}

#[test]
fn delete_doogat_cleans_materialized_row() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    exec_sql(&server, "CREATE TABLE bookmark (url VARCHAR(200))");

    let r = exec_sql(
        &server,
        "INSERT INTO bookmark (title, url) VALUES ('Delete Me', 'https://gone.com')",
    );
    let id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();

    // Verify materialized row exists
    let rows = select_objects(
        &server,
        &format!("SELECT url FROM bookmark WHERE id = '{id}'"),
    );
    assert_eq!(rows.len(), 1, "materialized row should exist before delete");
    assert_eq!(rows[0]["url"].as_str().unwrap(), "https://gone.com");

    // Delete via GraphQL
    let r = server.graphql(&format!(r#"mutation {{ deleteDoogat(id: "{id}") }}"#));
    assert!(r.get("errors").is_none(), "deleteDoogat failed: {r}");
    assert_eq!(r["data"]["deleteDoogat"], true);

    // Verify materialized row is gone
    let rows = select_objects(
        &server,
        &format!("SELECT url FROM bookmark WHERE id = '{id}'"),
    );
    assert!(
        rows.is_empty(),
        "materialized row should be removed after delete, got: {rows:?}"
    );
}

/// `updateDoogat` with a JSON-array value for a declared scalar column must
/// return a client-visible error and leave the materialized row unchanged.
/// This covers GraphQL transport JSON parsing; service-layer structured-value
/// validation is exercised by unit tests.
#[test]
fn update_doogat_with_json_array_field_returns_error_and_leaves_content_unchanged() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    exec_sql(&server, "CREATE TABLE note (content VARCHAR(500))");

    let r = exec_sql(
        &server,
        "INSERT INTO note (content) VALUES ('original content')",
    );
    let id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();

    let rows = select_objects(
        &server,
        &format!("SELECT content FROM note WHERE id = '{id}'"),
    );
    assert_eq!(rows.len(), 1, "initial materialized row must exist");
    assert_eq!(
        rows[0]["content"].as_str().unwrap_or(""),
        "original content",
        "initial content must match inserted value"
    );

    let r = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id } }"#,
        serde_json::json!({
            "input": {
                "id": id,
                "fields": "{\"content\":[\"item1\",\"item2\"]}"
            }
        }),
    );
    let error_message = r["errors"][0]["message"].as_str().unwrap_or("");
    assert!(
        error_message.contains("invalid fields JSON"),
        "updateDoogat with a JSON-array field value must return a fields JSON error; got: {r}"
    );

    let rows = select_objects(
        &server,
        &format!("SELECT content FROM note WHERE id = '{id}'"),
    );
    assert_eq!(rows.len(), 1, "row must still exist after failed update");
    assert_eq!(
        rows[0]["content"].as_str().unwrap_or(""),
        "original content",
        "content must be unchanged after rejected update"
    );
}
