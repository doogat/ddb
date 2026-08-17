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

fn create_doogat(server: &ServerGuard, input: Value) -> Value {
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title tags } }"#,
        json!({ "input": input }),
    );
    assert_graphql_ok(&result);
    result["data"]["createDoogat"].clone()
}

#[test]
fn integration_18c_graphql_tag_queries() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let created = create_doogat(
        &server,
        json!({ "title": "Tag Test", "tags": ["alpha", "beta"] }),
    );
    assert_eq!(created["title"], "Tag Test");
    assert_eq!(created["tags"], json!(["alpha", "beta"]));

    let result = server.graphql(r#"{ tags { name count } }"#);
    assert_graphql_ok(&result);
    let tags = result["data"]["tags"]
        .as_array()
        .expect("tags query missing array");
    for name in ["alpha", "beta"] {
        let tag = tags
            .iter()
            .find(|tag| tag["name"] == name)
            .unwrap_or_else(|| panic!("tags query missing {name}: {result}"));
        assert_eq!(tag["count"], 1);
    }
}

#[test]
fn integration_18c2_updated_at_created_at_fields() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let created = create_doogat(&server, json!({ "title": "Timestamp Test" }));
    let id = created["id"].as_str().expect("create missing id");

    let result = server.graphql_with_vars(
        r#"query($id: ID!) { doogat(id: $id) { updated_at created_at date } }"#,
        json!({ "id": id }),
    );
    assert_graphql_ok(&result);
    let doogat = &result["data"]["doogat"];
    assert!(
        doogat["updated_at"].is_string(),
        "missing updated_at: {result}"
    );
    assert!(
        doogat["created_at"].is_string(),
        "missing created_at: {result}"
    );
    assert_eq!(doogat["created_at"], doogat["date"]);

    let result =
        server.graphql(r#"{ search(query: "Timestamp Test") { hits { id updated_at } } }"#);
    assert_graphql_ok(&result);
    let hit = result["data"]["search"]["hits"]
        .as_array()
        .expect("search missing hits")
        .iter()
        .find(|hit| hit["id"] == id)
        .unwrap_or_else(|| panic!("timestamp search missing created id: {result}"));
    assert!(
        hit["updated_at"].is_string(),
        "search hit missing updated_at"
    );
}

#[test]
fn integration_18d_search_filter_by_type_and_tag() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(
        &server,
        "CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL, subtitle VARCHAR(255), favicon_path VARCHAR(255), favicon_origin VARCHAR(255), bookmark_source VARCHAR(255), last_opened_at VARCHAR(255), description TEXT)",
    );

    let alpha = create_doogat(
        &server,
        json!({
            "title": "SearchFilter Alpha",
            "type": "link",
            "tags": ["sf-tag"],
            "fields": "{\"url\":\"https://example.com/sf1\"}"
        }),
    );
    let beta = create_doogat(
        &server,
        json!({ "title": "SearchFilter Beta", "tags": ["sf-tag"] }),
    );
    let gamma = create_doogat(
        &server,
        json!({
            "title": "SearchFilter Gamma",
            "type": "link",
            "fields": "{\"url\":\"https://example.com/sf3\"}"
        }),
    );

    let result = server.graphql(
        r#"{ search(query: "SearchFilter", types: ["link"]) { totalCount hits { id } } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 2);
    let ids: Vec<&str> = result["data"]["search"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|hit| hit["id"].as_str())
        .collect();
    assert!(ids.contains(&alpha["id"].as_str().unwrap()));
    assert!(ids.contains(&gamma["id"].as_str().unwrap()));

    let result = server
        .graphql(r#"{ search(query: "SearchFilter", tag: "sf-tag") { totalCount hits { id } } }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 2);
    let ids: Vec<&str> = result["data"]["search"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|hit| hit["id"].as_str())
        .collect();
    assert!(ids.contains(&alpha["id"].as_str().unwrap()));
    assert!(ids.contains(&beta["id"].as_str().unwrap()));

    let result = server.graphql(
        r#"{ search(query: "SearchFilter", types: ["link"], tag: "sf-tag") { totalCount hits { id } } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 1);
    assert_eq!(result["data"]["search"]["hits"][0]["id"], alpha["id"]);
}

#[test]
fn integration_18g_normalize_query_implicit_and_lowercase() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(r#"{ normalizeSearchQuery(query: "  MEETING   Minutes  ") }"#);
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["normalizeSearchQuery"],
        "meeting and minutes"
    );
}

#[test]
fn integration_18h_in_query_field_filter_and_error_class() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let alpha = create_doogat(
        &server,
        json!({ "title": "PRD121 Alpha", "tags": ["prd121-rust"] }),
    );
    create_doogat(
        &server,
        json!({ "title": "PRD121 Beta", "tags": ["prd121-python"] }),
    );
    let gamma = create_doogat(
        &server,
        json!({
            "title": "PRD121 Gamma",
            "tags": ["prd121-rust", "prd121-cli"]
        }),
    );

    let result =
        server.graphql(r#"{ search(query: "tag=prd121-rust") { totalCount hits { id } } }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 2);
    let mut inline_ids: Vec<&str> = result["data"]["search"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|hit| hit["id"].as_str())
        .collect();
    inline_ids.sort_unstable();
    let mut expected = vec![alpha["id"].as_str().unwrap(), gamma["id"].as_str().unwrap()];
    expected.sort_unstable();
    assert_eq!(inline_ids, expected);

    let where_result = server.graphql(
        r#"{ search(query: "", where: [{field: "tag", eq: "prd121-rust"}]) { hits { id } } }"#,
    );
    assert_graphql_ok(&where_result);
    let mut where_ids: Vec<&str> = where_result["data"]["search"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|hit| hit["id"].as_str())
        .collect();
    where_ids.sort_unstable();
    assert_eq!(inline_ids, where_ids);

    let result = server.graphql(r#"{ search(query: "PRD121 tag=prd121-rust") { totalCount } }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 2);

    let result = server.graphql(
        r#"{ search(query: "tag=prd121-rust", where: [{field: "tag", eq: "prd121-python"}]) { totalCount } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 0);

    for malformed in ["*", "**", "(unbalanced", "AND"] {
        let result = server.graphql_with_vars(
            r#"query($query: String!) { search(query: $query) { totalCount } }"#,
            json!({ "query": malformed }),
        );
        let errors = result["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("{malformed:?} should fail: {result}"));
        let error_text = Value::Array(errors.clone()).to_string();
        assert!(
            error_text.contains("invalid search query"),
            "wrong error class for {malformed:?}: {result}"
        );
        assert!(
            !error_text.contains("internal error"),
            "internal error leaked for {malformed:?}: {result}"
        );
    }

    let result = server.graphql(r#"{ normalizeSearchQuery(query: "tag=prd121-rust") }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["normalizeSearchQuery"], "tag=prd121-rust");

    let result = server
        .graphql(r#"{ normalizeSearchQuery(query: "tag=prd121-rust AND category=work.dev") }"#);
    assert_graphql_ok(&result);
    let normalized = result["data"]["normalizeSearchQuery"]
        .as_str()
        .expect("normalization missing string");
    assert_eq!(normalized, "category=work.dev and tag=prd121-rust");
    let result = server.graphql_with_vars(
        r#"query($query: String!) { search(query: $query) { totalCount } }"#,
        json!({ "query": normalized }),
    );
    assert_graphql_ok(&result);
}

#[test]
fn integration_18i_in_query_substring_and_references_title() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(&server, "CREATE TABLE int133cat (label VARCHAR(100))");
    execute_sql(
        &server,
        "CREATE TABLE int133link (url TEXT, int133cat VARCHAR(14) REFERENCES int133cat(id))",
    );
    let category_id = execute_sql_id(
        &server,
        "INSERT INTO int133cat (title, label) VALUES ('Development', 'dev')",
    );
    execute_sql(
        &server,
        &format!(
            "INSERT INTO int133link (title, url, int133cat) VALUES ('Rust Async', 'https://example.com/rust-async', '{category_id}')"
        ),
    );
    execute_sql(
        &server,
        "INSERT INTO int133link (title, url) VALUES ('Meeting Notes Archive', 'https://example.com/archive')",
    );

    let result =
        server.graphql(r#"{ search(query: "title=Archive") { totalCount hits { title } } }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 1);
    assert_eq!(
        result["data"]["search"]["hits"][0]["title"],
        "Meeting Notes Archive"
    );

    let result = server
        .graphql(r#"{ search(query: "int133cat=Development") { totalCount hits { title } } }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 1);
    assert_eq!(result["data"]["search"]["hits"][0]["title"], "Rust Async");

    let result = server.graphql(
        r#"{ search(query: "", where: [{field: "int133cat", eq: "Development"}]) { totalCount } }"#,
    );
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 0);

    let result = server.graphql_with_vars(
        r#"query($id: String!) { search(query: "", where: [{field: "int133cat", eq: $id}]) { totalCount } }"#,
        json!({ "id": category_id }),
    );
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 1);
}

#[test]
fn integration_18z_update_delete_no_match_parity() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(&server, "CREATE TABLE link_b1 (url VARCHAR(255))");
    let id = execute_sql_id(
        &server,
        "INSERT INTO link_b1 (title, url) VALUES ('A', 'https://a.com')",
    );

    for sql in [
        "UPDATE link_b1 SET title = 'x' WHERE id = 'does_not_exist_b1'",
        "DELETE FROM link_b1 WHERE id = 'does_not_exist_b2'",
        "UPDATE link_b1 SET title = 'x' WHERE url = 'https://nope.com'",
        &format!("UPDATE link_b1 SET title = 'x' WHERE id = '{id}' AND url = 'https://wrong.com'"),
    ] {
        let result = execute_sql(&server, sql);
        assert_eq!(result["data"]["executeSql"]["affected"], 0, "{sql}");
    }

    let result = execute_sql(
        &server,
        &format!("UPDATE link_b1 SET title = 'from_in_clause' WHERE id IN ('nope', '{id}')"),
    );
    assert_eq!(result["data"]["executeSql"]["affected"], 1);

    let result = execute_sql(
        &server,
        &format!("UPDATE link_b1 SET title = 'final' WHERE id = '{id}'"),
    );
    assert_eq!(result["data"]["executeSql"]["affected"], 1);
}

#[test]
fn integration_18z2_execute_batch_atomicity() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(&server, "CREATE TABLE link_f4 (url VARCHAR(255))");
    execute_sql(
        &server,
        "CREATE TABLE membership_f4 (link_id VARCHAR(255), category VARCHAR(255), UNIQUE(link_id, category))",
    );
    let id = execute_sql_id(
        &server,
        "INSERT INTO link_f4 (title, url) VALUES ('initial', 'https://f4.com')",
    );
    execute_sql(
        &server,
        &format!(
            "INSERT INTO membership_f4 (title, link_id, category) VALUES ('m', '{id}', 'work')"
        ),
    );

    let result = server.graphql_with_vars(
        r#"mutation($statements: [String!]!) { executeBatch(statements: $statements) { message } }"#,
        json!({
            "statements": [
                format!("UPDATE link_f4 SET title = 'batched' WHERE id = '{id}' AND url = 'https://f4.com'"),
                format!("INSERT INTO membership_f4 (title, link_id, category) VALUES ('dup', '{id}', 'work')")
            ]
        }),
    );
    let errors = result["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("duplicate batch should fail: {result}"));
    assert!(Value::Array(errors.clone()).to_string().contains("UNIQUE"));

    let rows = sql_rows(
        &server,
        &format!("SELECT title FROM link_f4 WHERE id = '{id}'"),
    );
    assert_eq!(rows, vec![json!(["initial"])]);
}

#[test]
fn integration_18z3_update_doogat_tag_semantics() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let clear = create_doogat(
        &server,
        json!({ "title": "F5 tag clear", "tags": ["a", "b", "c"] }),
    );
    let result = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id } }"#,
        json!({ "input": { "id": clear["id"], "tags": [] } }),
    );
    assert_graphql_ok(&result);
    let result = server.graphql_with_vars(
        r#"query($id: ID!) { doogat(id: $id) { id tags } }"#,
        json!({ "id": clear["id"] }),
    );
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["doogat"]["tags"], json!([]));

    let dedupe = create_doogat(&server, json!({ "title": "F6 dedupe" }));
    let result = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id tags } }"#,
        json!({
            "input": {
                "id": dedupe["id"],
                "tags": ["x", "y", "x", "y", "x"]
            }
        }),
    );
    assert_graphql_ok(&result);
    let result = server.graphql_with_vars(
        r#"query($id: ID!) { doogat(id: $id) { id tags } }"#,
        json!({ "id": dedupe["id"] }),
    );
    assert_graphql_ok(&result);
    let mut tags: Vec<&str> = result["data"]["doogat"]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tag| tag.as_str().unwrap())
        .collect();
    tags.sort_unstable();
    assert_eq!(tags, vec!["x", "y"]);

    let unicode = create_doogat(
        &server,
        json!({ "title": "F7 unicode", "tags": ["日本語", "café", "ñoño"] }),
    );
    let result = server.graphql_with_vars(
        r#"query($id: ID!) { doogat(id: $id) { id tags } }"#,
        json!({ "id": unicode["id"] }),
    );
    assert_graphql_ok(&result);
    assert_eq!(
        result["data"]["doogat"]["tags"],
        json!(["日本語", "café", "ñoño"])
    );
}

#[test]
fn integration_18z4_sql_feature_coverage_pins() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(
        &server,
        "CREATE TABLE feat (val INTEGER, label VARCHAR(255), maybe_null VARCHAR(255))",
    );
    for sql in [
        "INSERT INTO feat (title, val, label, maybe_null) VALUES ('r1', 1, 'a', 'x')",
        "INSERT INTO feat (title, val, label) VALUES ('r2', 2, 'a')",
        "INSERT INTO feat (title, val, label, maybe_null) VALUES ('r3', 3, 'b', 'y')",
        "INSERT INTO feat (title, val, label) VALUES ('r4', 4, 'b')",
        "INSERT INTO feat (title, val, label, maybe_null) VALUES ('r5', 5, 'c', 'z')",
    ] {
        execute_sql(&server, sql);
    }

    assert_eq!(
        sql_rows(&server, "SELECT COUNT(*) FROM feat"),
        vec![json!(["5"])]
    );
    assert_eq!(
        sql_rows(
            &server,
            "SELECT label, COUNT(*) FROM feat GROUP BY label ORDER BY label"
        ),
        vec![json!(["a", "2"]), json!(["b", "2"]), json!(["c", "1"])]
    );
    assert_eq!(
        sql_rows(&server, "SELECT label FROM feat ORDER BY val DESC LIMIT 2"),
        vec![json!(["c"]), json!(["b"])]
    );
    assert_eq!(
        sql_rows(
            &server,
            "SELECT val FROM feat ORDER BY val ASC LIMIT 10 OFFSET 3"
        ),
        vec![json!(["4"]), json!(["5"])]
    );
    assert_eq!(
        sql_rows(
            &server,
            "SELECT COUNT(*) FROM feat WHERE maybe_null IS NULL"
        ),
        vec![json!(["2"])]
    );
    assert_eq!(
        sql_rows(&server, "SELECT COUNT(*) FROM feat WHERE label LIKE 'a%'"),
        vec![json!(["2"])]
    );
}

#[test]
fn integration_18z5_search_limit_boundary_pins() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    for title in ["F10boundary alpha", "F10boundary beta", "F10boundary gamma"] {
        create_doogat(&server, json!({ "title": title }));
    }

    let result =
        server.graphql(r#"{ search(query: "F10boundary", limit: 0) { totalCount hits { id } } }"#);
    assert!(!result.to_string().contains("internal error"), "{result}");

    let result = server
        .graphql(r#"{ search(query: "F10boundary", limit: 10000) { totalCount hits { id } } }"#);
    assert_graphql_ok(&result);
    assert_eq!(result["data"]["search"]["totalCount"], 3);

    let result = server.graphql(r#"{ search(query: "F10boundary", limit: 10001) { totalCount } }"#);
    let error_text = result["errors"].to_string();
    assert!(error_text.contains("limit must not exceed"), "{result}");
    assert!(!error_text.contains("internal error"), "{result}");

    let result = server.graphql(r#"{ search(query: "F10boundary", limit: -1) { totalCount } }"#);
    assert!(!result.to_string().contains("internal error"), "{result}");
}

#[test]
fn integration_18z6_alter_table_add_column_in_typedefs_introspection() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(&server, "CREATE TABLE altschema_f11 (a VARCHAR(255))");
    let before = server.graphql(r#"{ typeDefs { name columns { name dataType } } }"#);
    assert_graphql_ok(&before);
    let columns = before["data"]["typeDefs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|typedef| typedef["name"] == "altschema_f11")
        .expect("altschema_f11 missing before ALTER")["columns"]
        .as_array()
        .unwrap();
    assert!(columns.iter().any(|column| column["name"] == "a"));
    assert!(!columns.iter().any(|column| column["name"] == "b"));

    execute_sql(&server, "ALTER TABLE altschema_f11 ADD COLUMN b INTEGER");
    let after = server.graphql(r#"{ typeDefs { name columns { name dataType } } }"#);
    assert_graphql_ok(&after);
    let columns = after["data"]["typeDefs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|typedef| typedef["name"] == "altschema_f11")
        .expect("altschema_f11 missing after ALTER")["columns"]
        .as_array()
        .unwrap();
    assert!(columns.iter().any(|column| column["name"] == "a"));
    let added = columns
        .iter()
        .find(|column| column["name"] == "b")
        .expect("column b missing after ALTER");
    assert_eq!(added["dataType"], "INTEGER");
}

#[test]
fn integration_18z7_graphql_schema_introspection_contract() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(&server, "CREATE TABLE gqtesta (label VARCHAR(255))");
    execute_sql(&server, "CREATE TABLE gqtestb (label VARCHAR(255))");

    let result = server.graphql(
        r#"{ __schema { queryType { fields { name } } types { name fields { name } } } }"#,
    );
    assert_graphql_ok(&result);

    let query_fields: Vec<&str> = result["data"]["__schema"]["queryType"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|field| field["name"].as_str())
        .collect();
    for expected in [
        "gqtestas",
        "gqtestasAggregate",
        "gqtestbs",
        "gqtestbsAggregate",
    ] {
        assert!(
            query_fields.contains(&expected),
            "missing {expected}: {query_fields:?}"
        );
    }

    let types = result["data"]["__schema"]["types"].as_array().unwrap();
    for connection in ["GqtestaConnection", "GqtestbConnection"] {
        let ty = types
            .iter()
            .find(|ty| ty["name"] == connection)
            .unwrap_or_else(|| panic!("missing {connection}: {result}"));
        let fields: Vec<&str> = ty["fields"]
            .as_array()
            .expect("connection fields missing")
            .iter()
            .filter_map(|field| field["name"].as_str())
            .collect();
        assert!(fields.contains(&"items"), "{connection} missing items");
        assert!(
            fields.contains(&"totalCount"),
            "{connection} missing totalCount"
        );
    }
}
