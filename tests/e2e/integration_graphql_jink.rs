use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::{json, Value};

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

fn execute_sql(server: &ServerGuard, sql: &str) -> Value {
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message affected } }"#,
        json!({ "sql": sql }),
    );
    assert_graphql_ok(&result);
    result
}

fn execute_sql_id(server: &ServerGuard, sql: &str) -> String {
    execute_sql(server, sql)["data"]["executeSql"]["message"]
        .as_str()
        .expect("executeSql response missing id message")
        .trim()
        .to_string()
}

fn sql_rows(server: &ServerGuard, sql: &str) -> Vec<Value> {
    let result = server.graphql_with_vars(
        r#"query($sql: String!) { sql(query: $sql) { rows } }"#,
        json!({ "sql": sql }),
    );
    assert_graphql_ok(&result);
    result["data"]["sql"]["rows"]
        .as_array()
        .expect("sql response missing rows")
        .iter()
        .map(|row| {
            serde_json::from_str(row.as_str().expect("SQL row should be a JSON string"))
                .expect("SQL row should contain valid JSON")
        })
        .collect()
}

fn setup_jink_schema(server: &ServerGuard) {
    let tables = [
        (
            "link",
            "CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL, subtitle VARCHAR(255), favicon_path VARCHAR(255), favicon_origin VARCHAR(255), bookmark_source VARCHAR(255), last_opened_at VARCHAR(255), description TEXT)",
        ),
        (
            "category",
            "CREATE TABLE category (title VARCHAR(255) NOT NULL, fqn VARCHAR(255) NOT NULL, space VARCHAR(255) NOT NULL, sort_order INTEGER DEFAULT 0)",
        ),
        (
            "category-membership",
            "CREATE TABLE \"category-membership\" (title VARCHAR(255) NOT NULL, link_id VARCHAR(255) NOT NULL, category_fqn VARCHAR(255) NOT NULL, pinned BOOLEAN DEFAULT FALSE, sort_order INTEGER DEFAULT 0, UNIQUE(link_id, category_fqn))",
        ),
        (
            "quote",
            "CREATE TABLE quote (title VARCHAR(255) NOT NULL, author VARCHAR(255), source VARCHAR(255), favorited BOOLEAN DEFAULT FALSE, text TEXT)",
        ),
        (
            "saved-search",
            "CREATE TABLE \"saved-search\" (title VARCHAR(255) NOT NULL, query_raw VARCHAR(255) NOT NULL, query_normalized VARCHAR(255) NOT NULL)",
        ),
        (
            "pinned-result",
            "CREATE TABLE \"pinned-result\" (title VARCHAR(255) NOT NULL, query_normalized VARCHAR(255) NOT NULL, link_id VARCHAR(255) NOT NULL, sort_order INTEGER DEFAULT 0)",
        ),
        (
            "jink-config",
            "CREATE TABLE \"jink-config\" (dashboard_title VARCHAR(255) DEFAULT 'Bobs Battlestation', quote_rotation_minutes INTEGER DEFAULT 30, links_per_category INTEGER DEFAULT 8, frontend_version VARCHAR(255))",
        ),
    ];

    for (name, sql) in tables {
        let result = execute_sql(server, sql);
        let message = result["data"]["executeSql"]["message"]
            .as_str()
            .expect("CREATE TABLE response missing message");
        assert!(
            message.starts_with(&format!("table {name}")),
            "unexpected CREATE TABLE response for {name}: {result}"
        );
    }
}

fn seed_jink_config(server: &ServerGuard) -> String {
    execute_sql_id(
        server,
        "INSERT INTO \"jink-config\" (title, dashboard_title, quote_rotation_minutes, links_per_category) VALUES ('jink-config', 'Bobs Battlestation', 30, 8)",
    )
}

fn seed_jink_link(server: &ServerGuard) -> String {
    execute_sql_id(
        server,
        "INSERT INTO link (title, url, description) VALUES ('Test Link', 'https://example.com', 'a test link')",
    )
}

fn seed_jink_category_membership(server: &ServerGuard, link_id: &str) -> String {
    execute_sql_id(
        server,
        &format!(
            "INSERT INTO \"category-membership\" (title, link_id, category_fqn, sort_order) VALUES ('Test in work.dev', '{link_id}', 'work.dev', COALESCE((SELECT MAX(sort_order) + 1 FROM \"category-membership\" WHERE category_fqn = 'work.dev'), 0))"
        ),
    )
}

#[test]
fn integration_17j1_jink_schema_create_tables() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    setup_jink_schema(&server);

    let result = server.graphql(r#"{ typeDefs { name } }"#);
    assert_graphql_ok(&result);
    let names: Vec<&str> = result["data"]["typeDefs"]
        .as_array()
        .expect("typeDefs missing array")
        .iter()
        .filter_map(|typedef| typedef["name"].as_str())
        .collect();
    for expected in [
        "link",
        "category",
        "category-membership",
        "quote",
        "saved-search",
        "pinned-result",
        "jink-config",
    ] {
        assert!(
            names.contains(&expected),
            "missing jink type {expected}: {names:?}"
        );
    }
}

#[test]
fn integration_17j2_jink_config_singleton_and_link_crud() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_jink_schema(&server);

    assert!(
        sql_rows(&server, "SELECT id FROM \"jink-config\" LIMIT 1").is_empty(),
        "jink-config should initially be empty"
    );
    let config_id = seed_jink_config(&server);
    assert!(!config_id.is_empty());
    assert_eq!(
        sql_rows(
            &server,
            "SELECT quote_rotation_minutes FROM \"jink-config\" LIMIT 1"
        ),
        vec![json!(["30"])]
    );

    let link_id = seed_jink_link(&server);
    let result = server.graphql_with_vars(
        r#"query($id: String!) { links(where: {id: {eq: $id}}) { items { id title url description tags } } }"#,
        json!({ "id": link_id }),
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["links"]["items"],
        json!([{
            "id": link_id,
            "title": "Test Link",
            "url": "https://example.com",
            "description": "a test link",
            "tags": []
        }])
    );

    let result = execute_sql(
        &server,
        &format!(
            "UPDATE link SET favicon_path = 'favicon/x.png', favicon_origin = 'fetched' WHERE id = '{link_id}' AND url = 'https://example.com'"
        ),
    );
    assert_eq!(result["data"]["executeSql"]["affected"], 1);
}

#[test]
fn integration_18z8_jink_category_membership_port() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_jink_schema(&server);
    let link_id = seed_jink_link(&server);

    let category_id = execute_sql_id(
        &server,
        "INSERT INTO category (title, fqn, space, sort_order) VALUES ('Dev', 'work.dev', 'work', 0)",
    );
    assert!(!category_id.is_empty());

    let result = server.graphql(
        r#"{ categories(where: {space: {eq: "work"}}) { items { id fqn title space sort_order } } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["categories"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let category = &result["data"]["categories"]["items"][0];
    assert_eq!(category["id"], category_id);
    assert_eq!(category["fqn"], "work.dev");
    assert_eq!(category["title"], "Dev");
    assert_eq!(category["space"], "work");
    assert_eq!(category["sort_order"], 0);

    let result = server.graphql(
        r#"{ categories(where: {fqn: {in: ["work.dev"]}}) { items { fqn title space } } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["categories"]["items"],
        json!([{ "fqn": "work.dev", "title": "Dev", "space": "work" }])
    );

    let membership_id = seed_jink_category_membership(&server, &link_id);
    assert!(!membership_id.is_empty());

    let result = server.graphql_with_vars(
        r#"query($link: String!) { categoryMemberships(where: {link_id: {eq: $link}, category_fqn: {eq: "work.dev"}}) { items { id link_id category_fqn pinned sort_order } } }"#,
        json!({ "link": link_id }),
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["categoryMemberships"]["items"],
        json!([{
            "id": membership_id,
            "link_id": link_id,
            "category_fqn": "work.dev",
            "pinned": false,
            "sort_order": 0
        }])
    );

    let result = server.graphql_with_vars(
        r#"query($link: String!) { categoryMemberships(where: {link_id: {eq: $link}}) { items { category_fqn } } }"#,
        json!({ "link": link_id }),
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["categoryMemberships"]["items"],
        json!([{ "category_fqn": "work.dev" }])
    );

    let result = server.graphql(
        r#"{ categoryMemberships(where: {category_fqn: {eq: "work.dev"}}) { items { link_id pinned sort_order } } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["categoryMemberships"]["items"],
        json!([{ "link_id": link_id, "pinned": false, "sort_order": 0 }])
    );
}

#[test]
fn integration_18z9_jink_quotes_saved_searches_pinned_results_config_port() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_jink_schema(&server);
    seed_jink_config(&server);
    let link_id = seed_jink_link(&server);

    let quote_id = execute_sql_id(
        &server,
        "INSERT INTO quote (title, author, text) VALUES ('First', 'Anon', 'Hello world')",
    );
    let result = server.graphql_with_vars(
        r#"query($id: String!) { quotes(where: {id: {eq: $id}}) { items { id title text author } } }"#,
        json!({ "id": quote_id }),
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["quotes"]["items"],
        json!([{
            "id": quote_id,
            "title": "First",
            "text": "Hello world",
            "author": "Anon"
        }])
    );
    let result = server.graphql(r#"{ quotes { items { id } } }"#);
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["quotes"]["items"].as_array().unwrap().len(),
        1
    );
    let result = execute_sql(
        &server,
        &format!("UPDATE quote SET favorited = 'true' WHERE id = '{quote_id}' AND title = 'First'"),
    );
    assert_eq!(result["data"]["executeSql"]["affected"], 1);

    let saved_search_id = execute_sql_id(
        &server,
        "INSERT INTO \"saved-search\" (title, query_raw, query_normalized) VALUES ('rust stuff', 'Rust', 'rust')",
    );
    let result = server.graphql_with_vars(
        r#"query($id: String!) { savedSearches(where: {id: {eq: $id}}) { items { id title query_raw query_normalized } } }"#,
        json!({ "id": saved_search_id }),
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["savedSearches"]["items"],
        json!([{
            "id": saved_search_id,
            "title": "rust stuff",
            "query_raw": "Rust",
            "query_normalized": "rust"
        }])
    );

    let pinned_id = execute_sql_id(
        &server,
        &format!(
            "INSERT INTO \"pinned-result\" (title, query_normalized, link_id, sort_order) VALUES ('pinned test', 'rust', '{link_id}', 0)"
        ),
    );
    let result = server.graphql(
        r#"{ pinnedResults(where: {query_normalized: {eq: "rust"}}) { items { id query_normalized link_id sort_order } } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["pinnedResults"]["items"],
        json!([{
            "id": pinned_id,
            "query_normalized": "rust",
            "link_id": link_id,
            "sort_order": 0
        }])
    );

    let result = server.graphql(
        r#"{ jinkConfigs { items { id dashboard_title quote_rotation_minutes links_per_category frontend_version } } }"#,
    );
    assert_graphql_ok(&result);
    let config = &result["data"]["jinkConfigs"]["items"][0];
    assert_eq!(config["dashboard_title"], "Bobs Battlestation");
    assert_eq!(config["quote_rotation_minutes"], 30);
    assert_eq!(config["links_per_category"], 8);
    assert!(config["frontend_version"].is_null());

    let result = execute_sql(
        &server,
        "UPDATE \"jink-config\" SET frontend_version = '1.0.0' WHERE title = 'jink-config'",
    );
    assert_eq!(result["data"]["executeSql"]["affected"], 1);
    assert_eq!(
        sql_rows(
            &server,
            "SELECT frontend_version FROM \"jink-config\" LIMIT 1"
        ),
        vec![json!(["1.0.0"])]
    );
}

#[test]
fn integration_18z10_jink_composite_unique_and_batch_delete_port() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    setup_jink_schema(&server);
    let link_id = seed_jink_link(&server);
    execute_sql(
        &server,
        "INSERT INTO category (title, fqn, space, sort_order) VALUES ('Dev', 'work.dev', 'work', 0)",
    );
    seed_jink_category_membership(&server, &link_id);
    let quote_id = execute_sql_id(
        &server,
        "INSERT INTO quote (title, author, text) VALUES ('First', 'Anon', 'Hello world')",
    );

    let duplicate = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        json!({
            "sql": format!(
                "INSERT INTO \"category-membership\" (title, link_id, category_fqn) VALUES ('dup', '{link_id}', 'work.dev')"
            )
        }),
    );
    let errors = duplicate["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("duplicate membership should fail: {duplicate}"));
    assert!(Value::Array(errors.clone()).to_string().contains("UNIQUE"));

    let batch = server.graphql_with_vars(
        r#"mutation($statements: [String!]!) { executeBatch(statements: $statements) { message } }"#,
        json!({
            "statements": [
                format!("DELETE FROM \"category-membership\" WHERE link_id = '{link_id}' AND category_fqn = 'work.dev'"),
                format!("DELETE FROM link WHERE id = '{link_id}' AND url = 'https://example.com'")
            ]
        }),
    );
    assert_graphql_ok(&batch);
    assert!(batch["data"]["executeBatch"].is_array(), "{batch}");

    let result = server.graphql_with_vars(
        r#"query($id: String!) { links(where: {id: {eq: $id}}) { items { id } } }"#,
        json!({ "id": link_id }),
    );
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["links"]["items"], json!([]));

    let result = execute_sql(
        &server,
        &format!("DELETE FROM quote WHERE id = '{quote_id}' AND title = 'First'"),
    );
    assert_eq!(result["data"]["executeSql"]["affected"], 1);
}
