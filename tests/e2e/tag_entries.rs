use crate::common::{DdbTestRepo, ServerGuard};

#[test]
fn tag_entries_no_filter_returns_all() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "A", "tags": ["rust", "cli"] } }),
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "B", "tags": ["rust"] } }),
    );

    let result = server.graphql("{ tagEntries { items { doogatId tag source } totalCount } }");
    assert!(result.get("errors").is_none(), "query failed: {result}");
    let data = &result["data"]["tagEntries"];
    let total = data["totalCount"].as_i64().unwrap();
    assert_eq!(total, 3, "expected 3 tag entries: {data}");
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for item in items {
        assert!(item["doogatId"].as_str().is_some());
        assert!(item["tag"].as_str().is_some());
        assert_eq!(item["source"].as_str().unwrap(), "frontmatter");
    }
}

#[test]
fn tag_entries_filter_by_doogat_id() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r1 = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "A", "tags": ["rust", "cli"] } }),
    );
    let id_a = r1["data"]["createDoogat"]["id"].as_str().unwrap().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "B", "tags": ["python"] } }),
    );

    let query = format!(
        r#"{{ tagEntries(where: {{ doogatId: {{ eq: "{id_a}" }} }}) {{ items {{ doogatId tag }} totalCount }} }}"#
    );
    let result = server.graphql(&query);
    assert!(result.get("errors").is_none(), "query failed: {result}");
    let data = &result["data"]["tagEntries"];
    assert_eq!(data["totalCount"].as_i64().unwrap(), 2);
    for item in data["items"].as_array().unwrap() {
        assert_eq!(item["doogatId"].as_str().unwrap(), id_a);
    }
}

#[test]
fn tag_entries_filter_by_doogat_id_in() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r1 = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "A", "tags": ["rust"] } }),
    );
    let id_a = r1["data"]["createDoogat"]["id"].as_str().unwrap().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "B", "tags": ["python"] } }),
    );

    std::thread::sleep(std::time::Duration::from_secs(1));
    let r3 = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "C", "tags": ["go"] } }),
    );
    let id_c = r3["data"]["createDoogat"]["id"].as_str().unwrap().to_string();

    let query = format!(
        r#"{{ tagEntries(where: {{ doogatId: {{ in: ["{id_a}", "{id_c}"] }} }}) {{ items {{ doogatId tag }} totalCount }} }}"#
    );
    let result = server.graphql(&query);
    assert!(result.get("errors").is_none(), "query failed: {result}");
    let data = &result["data"]["tagEntries"];
    assert_eq!(data["totalCount"].as_i64().unwrap(), 2);
    let ids: Vec<&str> = data["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["doogatId"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&id_a.as_str()));
    assert!(ids.contains(&id_c.as_str()));
}

#[test]
fn tag_entries_filter_by_tag_eq() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "A", "tags": ["rust", "cli"] } }),
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "B", "tags": ["rust"] } }),
    );

    let result = server.graphql(
        r#"{ tagEntries(where: { tag: { eq: "rust" } }) { items { tag } totalCount } }"#,
    );
    assert!(result.get("errors").is_none(), "query failed: {result}");
    let data = &result["data"]["tagEntries"];
    assert_eq!(data["totalCount"].as_i64().unwrap(), 2);
    for item in data["items"].as_array().unwrap() {
        assert_eq!(item["tag"].as_str().unwrap(), "rust");
    }
}

#[test]
fn tag_entries_filter_by_tag_contains() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "A", "tags": ["client/acme", "client/beta"] } }),
    );
    std::thread::sleep(std::time::Duration::from_secs(1));
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "B", "tags": ["server"] } }),
    );

    let result = server.graphql(
        r#"{ tagEntries(where: { tag: { contains: "client" } }) { items { tag } totalCount } }"#,
    );
    assert!(result.get("errors").is_none(), "query failed: {result}");
    let data = &result["data"]["tagEntries"];
    assert_eq!(data["totalCount"].as_i64().unwrap(), 2);
    for item in data["items"].as_array().unwrap() {
        assert!(item["tag"].as_str().unwrap().contains("client"));
    }
}

#[test]
fn tag_entries_combined_filters() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r1 = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "A", "tags": ["rust", "cli"] } }),
    );
    let id_a = r1["data"]["createDoogat"]["id"].as_str().unwrap().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));
    server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        serde_json::json!({ "input": { "title": "B", "tags": ["rust", "python"] } }),
    );

    let query = format!(
        r#"{{ tagEntries(where: {{ doogatId: {{ eq: "{id_a}" }}, tag: {{ eq: "rust" }} }}) {{ items {{ doogatId tag }} totalCount }} }}"#
    );
    let result = server.graphql(&query);
    assert!(result.get("errors").is_none(), "query failed: {result}");
    let data = &result["data"]["tagEntries"];
    assert_eq!(data["totalCount"].as_i64().unwrap(), 1);
    let item = &data["items"][0];
    assert_eq!(item["doogatId"].as_str().unwrap(), id_a);
    assert_eq!(item["tag"].as_str().unwrap(), "rust");
}
