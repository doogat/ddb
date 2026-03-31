use crate::common::{ServerGuard, DdbTestRepo};
use std::time::Duration;

#[test]
fn auth_missing_token_returns_401() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = reqwest::blocking::Client::new()
        .post(server.url())
        .json(&serde_json::json!({ "query": "{ typeDefs { name } }" }))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");

    assert_eq!(resp.status(), 401);
}

#[test]
fn auth_wrong_token_returns_401() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = reqwest::blocking::Client::new()
        .post(server.url())
        .header("Authorization", "Bearer wrong-token")
        .json(&serde_json::json!({ "query": "{ typeDefs { name } }" }))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");

    assert_eq!(resp.status(), 401);
}

#[test]
fn crud_lifecycle() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title tags body } }"#,
        serde_json::json!({
            "input": {
                "title": "Test Note",
                "content": "Hello world",
                "tags": ["test", "graphql"]
            }
        }),
    );
    assert!(result.get("errors").is_none(), "create failed: {result}");
    let created = &result["data"]["createDoogat"];
    let id = created["id"].as_str().expect("missing id");
    assert!(!id.is_empty());
    assert_eq!(created["title"].as_str().unwrap(), "Test Note");
    assert_eq!(created["body"].as_str().unwrap(), "Hello world");

    // Read
    let result = server.graphql(&format!(
        r#"{{ doogat(id: "{id}") {{ id title body tags }} }}"#
    ));
    assert!(result.get("errors").is_none(), "read failed: {result}");
    let fetched = &result["data"]["doogat"];
    assert_eq!(fetched["title"].as_str().unwrap(), "Test Note");

    // Update
    let result = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id title body } }"#,
        serde_json::json!({
            "input": {
                "id": id,
                "title": "Updated Note",
                "content": "Updated body"
            }
        }),
    );
    assert!(result.get("errors").is_none(), "update failed: {result}");
    let updated = &result["data"]["updateDoogat"];
    assert_eq!(updated["title"].as_str().unwrap(), "Updated Note");
    assert_eq!(updated["body"].as_str().unwrap(), "Updated body");

    // Delete
    let result = server.graphql(&format!(r#"mutation {{ deleteDoogat(id: "{id}") }}"#));
    assert!(result.get("errors").is_none(), "delete failed: {result}");
    assert_eq!(result["data"]["deleteDoogat"], true);

    // Verify deleted
    let result = server.graphql(&format!(r#"{{ doogat(id: "{id}") {{ id }} }}"#));
    assert!(result["errors"].is_array());
}

#[test]
fn search_and_list() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a few doogats
    let r1 = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Alpha Note", "content": "searchable content", "tags": ["alpha"] } }),
    );
    assert!(r1.get("errors").is_none(), "create alpha failed: {r1}");

    let r2 = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Beta Note", "content": "different content", "tags": ["beta"] } }),
    );
    assert!(r2.get("errors").is_none(), "create beta failed: {r2}");

    // List all
    let result = server.graphql(r#"{ doogats { id title } }"#);
    assert!(result.get("errors").is_none(), "list all failed: {result}");
    let list = result["data"]["doogats"].as_array().unwrap();
    assert!(list.len() >= 2);

    // List by tag
    let result = server.graphql(r#"{ doogats(tag: "alpha") { id title } }"#);
    assert!(
        result.get("errors").is_none(),
        "list by tag failed: {result}"
    );
    let list = result["data"]["doogats"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["title"].as_str().unwrap(), "Alpha Note");

    // Search
    let result = server
        .graphql(r#"{ search(query: "searchable") { hits { id title snippet } totalCount } }"#);
    assert!(result.get("errors").is_none(), "search failed: {result}");
    let hits = result["data"]["search"]["hits"].as_array().unwrap();
    assert!(!hits.is_empty());

    // typeDefs (empty, no types installed)
    let result = server.graphql(r#"{ typeDefs { name } }"#);
    assert!(result.get("errors").is_none(), "typeDefs failed: {result}");
    let defs = result["data"]["typeDefs"].as_array().unwrap();
    assert!(defs.is_empty());
}

#[test]
fn sql_query() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a doogat
    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "SQL Test", "content": "body" } }),
    );
    assert!(r.get("errors").is_none(), "create failed: {r}");

    // SQL query
    let result =
        server.graphql(r#"{ sql(query: "SELECT id, title FROM doogats") { rows message } }"#);
    assert!(result.get("errors").is_none(), "sql query failed: {result}");
    let sql = &result["data"]["sql"];
    let rows = sql["rows"].as_array().unwrap();
    assert!(!rows.is_empty());
}

// ── REST API tests ──────────────────────────────────────────────

#[test]
fn rest_crud_lifecycle() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create
    let resp = server.rest_post(
        "/doogats",
        serde_json::json!({
            "title": "REST Note",
            "body": "Hello REST",
            "tags": ["rest", "test"]
        }),
    );
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().unwrap();
    let id = created["data"]["id"].as_str().expect("missing id");
    assert!(!id.is_empty());
    assert_eq!(created["data"]["title"].as_str().unwrap(), "REST Note");
    assert_eq!(created["data"]["body"].as_str().unwrap(), "Hello REST");

    // Read
    let resp = server.rest_get(&format!("/doogats/{id}"));
    assert_eq!(resp.status(), 200);
    let fetched: serde_json::Value = resp.json().unwrap();
    assert_eq!(fetched["data"]["title"].as_str().unwrap(), "REST Note");

    // Update
    let resp = server.rest_put(
        &format!("/doogats/{id}"),
        serde_json::json!({
            "title": "Updated REST Note",
            "body": "Updated body"
        }),
    );
    assert_eq!(resp.status(), 200);
    let updated: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        updated["data"]["title"].as_str().unwrap(),
        "Updated REST Note"
    );
    assert_eq!(updated["data"]["body"].as_str().unwrap(), "Updated body");

    // Delete
    let resp = server.rest_delete(&format!("/doogats/{id}"));
    assert_eq!(resp.status(), 204);

    // Verify deleted
    let resp = server.rest_get(&format!("/doogats/{id}"));
    assert_eq!(resp.status(), 404);
}

#[test]
fn rest_pagination() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create 3 doogats
    for i in 0..3 {
        let resp = server.rest_post(
            "/doogats",
            serde_json::json!({ "title": format!("Page Note {i}") }),
        );
        assert_eq!(resp.status(), 201);
    }

    // Page 1 with per_page=2
    let resp = server.rest_get("/doogats?per_page=2&page=1");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert_eq!(body["pagination"]["page"].as_i64().unwrap(), 1);
    assert_eq!(body["pagination"]["per_page"].as_i64().unwrap(), 2);
    assert_eq!(body["pagination"]["total"].as_i64().unwrap(), 3);
    assert_eq!(body["pagination"]["total_pages"].as_i64().unwrap(), 2);

    // Page 2
    let resp = server.rest_get("/doogats?per_page=2&page=2");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[test]
fn rest_filter_by_tag() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    server.rest_post(
        "/doogats",
        serde_json::json!({ "title": "Tagged", "tags": ["alpha"] }),
    );
    server.rest_post("/doogats", serde_json::json!({ "title": "Untagged" }));

    let resp = server.rest_get("/doogats?tag=alpha");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["title"].as_str().unwrap(), "Tagged");
}

#[test]
fn rest_search() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r = server.rest_post(
        "/doogats",
        serde_json::json!({ "title": "Findable", "body": "searchable content here" }),
    );
    assert_eq!(r.status(), 201, "create failed");

    let resp = server.rest_get("/doogats?q=searchable");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert!(data[0].get("snippet").is_some());
}

#[test]
fn rest_filter_by_field() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a typed table with a frontmatter field (INTEGER → frontmatter → _ddb_fields)
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE item (name TEXT NOT NULL, priority INTEGER)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE failed: {r}");

    // Insert rows with different priorities
    for (name, priority) in [("Alpha", 1), ("Beta", 2), ("Gamma", 1)] {
        let sql = format!("INSERT INTO item (name, priority) VALUES ('{name}', {priority})");
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": sql }),
        );
        assert!(r.get("errors").is_none(), "INSERT failed: {r}");
    }

    // Filter by field.priority=1 → should return Alpha and Gamma
    let resp = server.rest_get("/doogats?field.priority=1");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2, "expected 2 matches, got: {data:?}");

    // Filter by field.priority=2 → Beta only
    let resp = server.rest_get("/doogats?field.priority=2");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1, "expected 1 match, got: {data:?}");

    // Nonexistent value → empty
    let resp = server.rest_get("/doogats?field.priority=99");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert!(data.is_empty());

    // Multiple field filters AND together
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "ALTER TABLE item ADD COLUMN status INTEGER" }),
    );
    assert!(r.get("errors").is_none(), "ALTER TABLE failed: {r}");
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO item (name, priority, status) VALUES ('Delta', 1, 10)" }),
    );
    assert!(r.get("errors").is_none(), "INSERT failed: {r}");

    // Delta has priority=1 AND status=10; Alpha and Gamma have priority=1 but no status
    let resp = server.rest_get("/doogats?field.priority=1&field.status=10");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(
        data.len(),
        1,
        "multi-field AND: expected 1 match, got: {data:?}"
    );

    // SQL injection via field key — should return empty, not error
    let resp = server.rest_get("/doogats?field.';DROP%20TABLE%20doogats--=x");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert!(data.is_empty(), "SQL injection attempt should return empty");

    // Verify doogats table still exists
    let resp = server.rest_get("/doogats");
    assert_eq!(resp.status(), 200);
}

#[test]
fn rest_filter_field_and_tag() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create tagged doogat with extra field via SQL type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE widget (label TEXT NOT NULL, priority INTEGER)" }),
    );
    assert!(r.get("errors").is_none());
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO widget (label, priority) VALUES ('W1', 5)" }),
    );
    assert!(r.get("errors").is_none());

    // Create untyped doogat (no priority field)
    server.rest_post(
        "/doogats",
        serde_json::json!({ "title": "Plain", "tags": ["widget"] }),
    );

    // Filter by type + field → only the SQL-created widget
    let resp = server.rest_get("/doogats?type=widget&field.priority=5");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(
        data.len(),
        1,
        "type+field filter: expected 1, got: {data:?}"
    );
}

#[test]
fn rest_auth_required() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // No auth header
    let resp = server
        .rest_client()
        .get(server.rest_url("/doogats"))
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Wrong token
    let resp = server
        .rest_client()
        .get(server.rest_url("/doogats"))
        .header("Authorization", "Bearer wrong-token")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// ── Hot schema reload tests ─────────────────────────────────────

#[test]
fn hot_schema_reload_create_and_query() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Verify schemaVersion works (reloader is in schema data)
    let result = server.graphql(r#"{ schemaVersion }"#);
    assert!(
        result.get("errors").is_none(),
        "initial schemaVersion failed: {result}"
    );
    let v1 = result["data"]["schemaVersion"].as_i64().unwrap();
    assert_eq!(v1, 1, "initial schemaVersion should be 1");

    // Create a new type at runtime
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE book (title TEXT NOT NULL, author TEXT)" }),
    );
    assert!(
        result.get("errors").is_none(),
        "CREATE TABLE failed: {result}"
    );

    // Schema reload is synchronous — new type is immediately queryable
    let result = server.graphql(r#"{ books { items { id title } totalCount } }"#);
    assert!(
        result.get("errors").is_none(),
        "books query failed after reload: {result}"
    );
    let books = result["data"]["books"]["items"].as_array().unwrap();
    assert!(books.is_empty());
    assert_eq!(result["data"]["books"]["totalCount"].as_i64().unwrap(), 0);

    // Schema version should have incremented
    let result = server.graphql(r#"{ schemaVersion }"#);
    assert!(
        result.get("errors").is_none(),
        "schemaVersion query failed: {result}"
    );
    let version = result["data"]["schemaVersion"].as_i64().unwrap();
    assert!(
        version > 1,
        "schemaVersion should be >1 after reload, got {version}"
    );
}

#[test]
fn hot_schema_reload_schema_version_increments() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Initial version
    let result = server.graphql(r#"{ schemaVersion }"#);
    assert!(
        result.get("errors").is_none(),
        "schemaVersion failed: {result}"
    );
    let v1 = result["data"]["schemaVersion"].as_i64().unwrap();

    // Create type → triggers reload
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE book (title TEXT NOT NULL)" }),
    );
    assert!(
        result.get("errors").is_none(),
        "CREATE TABLE failed: {result}"
    );

    // Version should have incremented
    let result = server.graphql(r#"{ schemaVersion }"#);
    assert!(
        result.get("errors").is_none(),
        "schemaVersion failed: {result}"
    );
    let v2 = result["data"]["schemaVersion"].as_i64().unwrap();
    assert!(v2 > v1, "schemaVersion should increment: {v1} → {v2}");
}

#[test]
fn hot_schema_reload_multiple_creates() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    for table in ["book", "movie", "song"] {
        let sql = format!("CREATE TABLE {table} (title TEXT NOT NULL)");
        let result = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": sql }),
        );
        assert!(
            result.get("errors").is_none(),
            "CREATE TABLE {table} failed: {result}"
        );
    }

    // All 3 types should be queryable (reload is synchronous)
    for (query, name) in [
        (r#"{ books { items { id } totalCount } }"#, "books"),
        (r#"{ movies { items { id } totalCount } }"#, "movies"),
        (r#"{ songs { items { id } totalCount } }"#, "songs"),
    ] {
        let result = server.graphql(query);
        assert!(
            result.get("errors").is_none(),
            "{name} query failed: {result}"
        );
    }
}

#[test]
fn drop_table_removes_type_from_schema() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a type
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE book (title TEXT NOT NULL)" }),
    );
    assert!(
        result.get("errors").is_none(),
        "CREATE TABLE failed: {result}"
    );

    // DROP TABLE removes typedef doogat and triggers schema reload
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "DROP TABLE book" }),
    );
    assert!(
        result.get("errors").is_none(),
        "DROP TABLE should not error: {result}"
    );

    // Type is no longer in schema
    let result = server.graphql(r#"{ books { items { id } totalCount } }"#);
    assert!(
        result.get("errors").is_some(),
        "books should no longer be queryable after DROP: {result}"
    );
}

// ── Filtering, sorting, aggregation tests ──────────────────────

/// Helper: create a "task" type with status (TEXT) + priority (INTEGER), insert test rows.
fn setup_task_type(server: &ServerGuard) {
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE task (status TEXT NOT NULL, priority INTEGER)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE task failed: {r}");

    for (status, priority) in [("open", 1), ("open", 3), ("closed", 2), ("review", 3)] {
        let sql = format!("INSERT INTO task (status, priority) VALUES ('{status}', {priority})");
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": sql }),
        );
        assert!(r.get("errors").is_none(), "INSERT failed: {r}");
        std::thread::sleep(Duration::from_secs(1)); // avoid ID collision
    }
}

#[test]
fn filter_eq() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    let result = server.graphql(
        r#"{ tasks(where: { status: { eq: "open" } }) { items { id status } totalCount } }"#,
    );
    assert!(result.get("errors").is_none(), "filter eq failed: {result}");
    let items = result["data"]["tasks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 2);
    for item in items {
        assert_eq!(item["status"].as_str().unwrap(), "open");
    }
}

#[test]
fn filter_gte() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    let result = server.graphql(
        r#"{ tasks(where: { priority: { gte: 3 } }) { items { id priority } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "filter gte failed: {result}"
    );
    let items = result["data"]["tasks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 2);
    for item in items {
        assert!(item["priority"].as_i64().unwrap() >= 3);
    }
}

#[test]
fn filter_contains() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    let result = server.graphql(
        r#"{ tasks(where: { status: { contains: "ope" } }) { items { id status } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "filter contains failed: {result}"
    );
    let items = result["data"]["tasks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        assert!(item["status"].as_str().unwrap().contains("ope"));
    }
}

#[test]
fn filter_compound_and_or() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    // _or: status=open OR status=review → 3 results
    let result = server.graphql(
        r#"{ tasks(where: { _or: [{ status: { eq: "open" } }, { status: { eq: "review" } }] }) { items { id } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "filter _or failed: {result}"
    );
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 3);

    // _and: status=open AND priority>=3 → 1 result
    let result = server.graphql(
        r#"{ tasks(where: { _and: [{ status: { eq: "open" } }, { priority: { gte: 3 } }] }) { items { id } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "filter _and failed: {result}"
    );
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 1);
}

#[test]
fn order_by() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    let result = server
        .graphql(r#"{ tasks(orderBy: { priority: ASC }) { items { priority } totalCount } }"#);
    assert!(result.get("errors").is_none(), "orderBy failed: {result}");
    let items = result["data"]["tasks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    let priorities: Vec<i64> = items
        .iter()
        .map(|i| i["priority"].as_i64().unwrap())
        .collect();
    assert_eq!(
        priorities,
        vec![1, 2, 3, 3],
        "should be sorted ASC: {priorities:?}"
    );
}

#[test]
fn aggregate_query() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    let result = server.graphql(r#"{ tasksAggregate { count minPriority maxPriority } }"#);
    assert!(result.get("errors").is_none(), "aggregate failed: {result}");
    let agg = &result["data"]["tasksAggregate"];
    assert_eq!(agg["count"].as_i64().unwrap(), 4);
    assert_eq!(agg["minPriority"].as_f64().unwrap() as i64, 1);
    assert_eq!(agg["maxPriority"].as_f64().unwrap() as i64, 3);
}

#[test]
fn aggregate_with_filter() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    let result =
        server.graphql(r#"{ tasksAggregate(where: { status: { eq: "open" } }) { count } }"#);
    assert!(
        result.get("errors").is_none(),
        "aggregate with filter failed: {result}"
    );
    assert_eq!(
        result["data"]["tasksAggregate"]["count"].as_i64().unwrap(),
        2
    );
}

#[test]
fn filter_sql_injection_attempt() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    // Attempt SQL injection via filter value
    let result = server.graphql(
        r#"{ tasks(where: { status: { eq: "'; DROP TABLE task; --" } }) { items { id } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "injection attempt should not error: {result}"
    );
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 0);

    // Verify table still works
    let result = server.graphql(r#"{ tasks { items { id } totalCount } }"#);
    assert!(
        result.get("errors").is_none(),
        "tasks should still work after injection attempt: {result}"
    );
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 4);
}

#[test]
fn filter_tag_with_where() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_task_type(&server);

    // Get all task IDs and their statuses
    let result = server.graphql(
        r#"{ tasks(orderBy: { priority: ASC }) { items { id status priority } totalCount } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "list tasks failed: {result}"
    );
    let items = result["data"]["tasks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);

    // Tag the first two items (priority 1 and 2) with "urgent"
    for item in &items[..2] {
        let id = item["id"].as_str().unwrap();
        let r = server.graphql_with_vars(
            r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id tags } }"#,
            serde_json::json!({ "input": { "id": id, "tags": ["urgent"] } }),
        );
        assert!(r.get("errors").is_none(), "tag update failed: {r}");
    }

    // tag="urgent" + where filter: should return only tagged items matching the where
    let result = server.graphql(
        r#"{ tasks(tag: "urgent", where: { priority: { gte: 2 } }) { items { id priority } totalCount } }"#,
    );
    assert!(result.get("errors").is_none(), "tag+where failed: {result}");
    let items = result["data"]["tasks"]["items"].as_array().unwrap();
    // Only the priority=2 item is tagged "urgent" AND has priority >= 2
    assert_eq!(items.len(), 1, "expected 1 item with tag+where: {result}");
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 1);

    // tag="urgent" alone: should return both tagged items
    let result = server.graphql(r#"{ tasks(tag: "urgent") { items { id } totalCount } }"#);
    assert!(result.get("errors").is_none(), "tag-only failed: {result}");
    assert_eq!(
        result["data"]["tasks"]["items"].as_array().unwrap().len(),
        2
    );
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 2);

    // where alone (no tag): should return all matching regardless of tag
    let result =
        server.graphql(r#"{ tasks(where: { priority: { gte: 2 } }) { items { id } totalCount } }"#);
    assert!(
        result.get("errors").is_none(),
        "where-only failed: {result}"
    );
    assert_eq!(
        result["data"]["tasks"]["items"].as_array().unwrap().len(),
        3
    );
    assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 3);
}

#[test]
fn alter_table_column_visible_in_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a type with one column
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE note (title TEXT NOT NULL)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE failed: {r}");

    // title is queryable
    let r = server.graphql(r#"{ notes { items { id title } totalCount } }"#);
    assert!(r.get("errors").is_none(), "notes query failed: {r}");

    // ADD COLUMN — new column immediately visible
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "ALTER TABLE note ADD COLUMN priority INTEGER" }),
    );
    assert!(
        r.get("errors").is_none(),
        "ALTER TABLE ADD COLUMN failed: {r}"
    );

    let r = server.graphql(r#"{ notes { items { id title priority } totalCount } }"#);
    assert!(
        r.get("errors").is_none(),
        "priority column should be visible after ALTER: {r}"
    );

    // DROP COLUMN — removed column no longer queryable
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "ALTER TABLE note DROP COLUMN priority" }),
    );
    assert!(
        r.get("errors").is_none(),
        "ALTER TABLE DROP COLUMN failed: {r}"
    );

    let r = server.graphql(r#"{ notes { items { id title priority } totalCount } }"#);
    assert!(
        r.get("errors").is_some(),
        "priority should not be queryable after DROP: {r}"
    );
}

#[test]
fn malformed_typedef_preserves_schema() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a valid type first
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE widget (label TEXT NOT NULL)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE failed: {r}");

    // Verify it works
    let r = server.graphql(r#"{ widgets { items { id label } totalCount } }"#);
    assert!(r.get("errors").is_none(), "widgets query failed: {r}");

    // Write a malformed typedef directly (invalid YAML frontmatter)
    let typedef_content =
        "---\ntype: _typedef\ntable_name: broken\ncolumns:\n  - bad yaml {{{\n---\n";
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "body": typedef_content, "tags": ["_typedef"] } }),
    );

    // Previous schema still intact — widgets still queryable
    let r = server.graphql(r#"{ widgets { items { id label } totalCount } }"#);
    assert!(
        r.get("errors").is_none(),
        "widgets should still be queryable after malformed typedef: {r}"
    );

    // Server is still responsive
    let r = server.graphql(r#"{ schemaVersion }"#);
    assert!(
        r.get("errors").is_none(),
        "server should still respond: {r}"
    );
}

// ── Health endpoint tests ───────────────────────────────────────

#[test]
fn health_returns_ok() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/health", server.port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("health request failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ok");
    assert!(body["version"].as_str().is_some());
    assert!(body["uptime_seconds"].as_u64().is_some());
    assert!(body["index_reachable"].as_bool().unwrap());
}

#[test]
fn health_no_auth_required() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // No Authorization header — should still get 200, not 401
    let resp = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/health", server.port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("health request failed");

    assert_eq!(resp.status(), 200);
}

#[test]
fn health_live_always_ok() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/health/live", server.port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("health/live request failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ok");
}

// ── SQL object-format tests ────────────────────────────────────

#[test]
fn sql_format_objects_returns_keyed_rows() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a doogat so there's data to query
    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Obj Test", "content": "body" } }),
    );
    assert!(r.get("errors").is_none(), "create failed: {r}");
    let id = r["data"]["createDoogat"]["id"].as_str().unwrap();

    // Query with format:"objects" — rows should be JSON objects keyed by column name
    let result = server.graphql(
        r#"{ sql(query: "SELECT id, title FROM doogats", format: "objects") { columns rows } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "sql with format:objects failed: {result}"
    );
    let sql = &result["data"]["sql"];
    let rows = sql["rows"].as_array().expect("rows should be an array");
    assert!(!rows.is_empty(), "expected at least one row");

    // Each row should be a JSON object with "id" and "title" keys
    let row: serde_json::Value =
        serde_json::from_str(rows[0].as_str().expect("row should be a string"))
            .expect("row should be valid JSON");
    assert!(row.is_object(), "row should be a JSON object, got: {row}");
    assert_eq!(row["id"].as_str().unwrap(), id);
    assert_eq!(row["title"].as_str().unwrap(), "Obj Test");
}

#[test]
fn sql_default_format_returns_arrays() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a doogat
    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Array Test", "content": "body" } }),
    );
    assert!(r.get("errors").is_none(), "create failed: {r}");

    // Query without format arg — rows should be JSON arrays (backwards compat)
    let result =
        server.graphql(r#"{ sql(query: "SELECT id, title FROM doogats") { columns rows } }"#);
    assert!(
        result.get("errors").is_none(),
        "sql without format failed: {result}"
    );
    let sql = &result["data"]["sql"];
    let rows = sql["rows"].as_array().expect("rows should be an array");
    assert!(!rows.is_empty(), "expected at least one row");

    // Each row should be a JSON array, not an object
    let row: serde_json::Value =
        serde_json::from_str(rows[0].as_str().expect("row should be a string"))
            .expect("row should be valid JSON");
    assert!(
        row.is_array(),
        "default format row should be a JSON array, got: {row}"
    );
}

#[test]
fn execute_sql_format_objects() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a typed table and insert data
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE item (name TEXT NOT NULL, price INTEGER)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO item (name, price) VALUES ('Widget', 42)" }),
    );
    assert!(r.get("errors").is_none(), "INSERT failed: {r}");

    // executeSql SELECT with format:"objects"
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!, $fmt: String) { executeSql(sql: $sql, format: $fmt) { columns rows } }"#,
        serde_json::json!({ "sql": "SELECT name, price FROM item", "fmt": "objects" }),
    );
    assert!(
        result.get("errors").is_none(),
        "executeSql with format:objects failed: {result}"
    );
    let sql = &result["data"]["executeSql"];
    let rows = sql["rows"].as_array().expect("rows should be an array");
    assert!(!rows.is_empty(), "expected at least one row");

    let row: serde_json::Value =
        serde_json::from_str(rows[0].as_str().expect("row should be a string"))
            .expect("row should be valid JSON");
    assert!(row.is_object(), "row should be a JSON object, got: {row}");
    assert_eq!(row["name"].as_str().unwrap(), "Widget");
    assert_eq!(row["price"].as_str().unwrap(), "42");
}

#[test]
fn non_select_with_format_objects_ignored() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a typed table
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE thing (name TEXT NOT NULL)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE failed: {r}");

    // INSERT with format:"objects" — should succeed, format is ignored for non-SELECT
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!, $fmt: String) { executeSql(sql: $sql, format: $fmt) { affected message } }"#,
        serde_json::json!({ "sql": "INSERT INTO thing (name) VALUES ('A')", "fmt": "objects" }),
    );
    assert!(
        result.get("errors").is_none(),
        "INSERT with format:objects should not error: {result}"
    );
    let sql = &result["data"]["executeSql"];
    // Non-SELECT returns message with the created ID
    assert!(sql["message"].is_string());
}

#[test]
fn sql_columns_aliased() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a doogat so there's data to query
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Alias Test" } }),
    );

    // Aliased column name should appear in columns
    let result = server.graphql(
        r#"{ sql(query: "SELECT id AS doogat_id, title AS name FROM doogats") { columns } }"#,
    );
    assert!(result.get("errors").is_none(), "aliased query failed: {result}");
    let cols = result["data"]["sql"]["columns"].as_array().unwrap();
    assert_eq!(cols[0].as_str().unwrap(), "doogat_id");
    assert_eq!(cols[1].as_str().unwrap(), "name");
}

#[test]
fn sql_columns_star_select() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a typed table and insert a row
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE gadget (label TEXT NOT NULL, weight INTEGER)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE TABLE failed: {r}");
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO gadget (label, weight) VALUES ('G1', 10)" }),
    );
    assert!(r.get("errors").is_none(), "INSERT failed: {r}");

    // SELECT * should return all columns
    let result = server.graphql(
        r#"{ sql(query: "SELECT * FROM gadget") { columns rows } }"#,
    );
    assert!(result.get("errors").is_none(), "star select failed: {result}");
    let cols = result["data"]["sql"]["columns"].as_array().unwrap();
    let col_names: Vec<&str> = cols.iter().map(|c| c.as_str().unwrap()).collect();
    assert!(col_names.contains(&"label"), "missing label column: {col_names:?}");
    assert!(col_names.contains(&"weight"), "missing weight column: {col_names:?}");
}

#[test]
fn execute_batch_format_objects() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create type and insert data
    server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE planet (name TEXT NOT NULL)" }),
    );
    server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO planet (name) VALUES ('Mars')" }),
    );

    // executeBatch with format:"objects"
    let result = server.graphql_with_vars(
        r#"mutation($stmts: [String!]!, $fmt: String) { executeBatch(statements: $stmts, format: $fmt) { columns rows } }"#,
        serde_json::json!({ "stmts": ["SELECT name FROM planet"], "fmt": "objects" }),
    );
    assert!(result.get("errors").is_none(), "executeBatch format:objects failed: {result}");
    let batch = result["data"]["executeBatch"].as_array().unwrap();
    assert!(!batch.is_empty());
    let rows = batch[0]["rows"].as_array().unwrap();
    assert!(!rows.is_empty());
    let row: serde_json::Value = serde_json::from_str(rows[0].as_str().unwrap()).unwrap();
    assert!(row.is_object(), "batch row should be object: {row}");
    assert_eq!(row["name"].as_str().unwrap(), "Mars");
}

#[test]
fn search_boolean_and_phrase_queries() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create doogats with distinct content
    for (title, body) in [
        ("Rust CRDT Guide", "rust crdt patterns"),
        ("Rust Only", "rust programming basics"),
        ("Golang Guide", "golang programming"),
    ] {
        let r = server.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": { "title": title, "content": body } }),
        );
        assert!(r.get("errors").is_none(), "create {title} failed: {r}");
    }

    // AND: only the doogat with both terms
    let result = server.graphql(
        r#"{ search(query: "rust AND crdt") { totalCount hits { title } } }"#,
    );
    assert!(result.get("errors").is_none(), "AND query failed: {result}");
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 1);

    // OR: doogats with either term
    let result = server.graphql(
        r#"{ search(query: "rust OR golang") { totalCount } }"#,
    );
    assert!(result.get("errors").is_none(), "OR query failed: {result}");
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 3);

    // NOT: rust without crdt
    let result = server.graphql(
        r#"{ search(query: "rust NOT crdt") { totalCount } }"#,
    );
    assert!(result.get("errors").is_none(), "NOT query failed: {result}");
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 1);

    // Quoted phrase
    let result = server.graphql(
        r#"{ search(query: "\"rust crdt\"") { totalCount } }"#,
    );
    assert!(result.get("errors").is_none(), "phrase query failed: {result}");
    assert_eq!(result["data"]["search"]["totalCount"].as_i64().unwrap(), 1);
}

#[test]
fn search_malformed_query_returns_bad_request() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create at least one doogat so the index isn't empty
    let r = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "Dummy", "content": "content" } }),
    );
    assert!(r.get("errors").is_none(), "create failed: {r}");

    let result = server.graphql(
        r#"{ search(query: "AND AND") { totalCount } }"#,
    );
    assert!(result["errors"].is_array(), "expected errors for malformed query: {result}");
    let err_msg = result["errors"][0]["message"].as_str().unwrap();
    assert!(err_msg.contains("invalid search query"), "expected user-facing error, got: {err_msg}");
}
