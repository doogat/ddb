use crate::common::{DdbTestRepo, ServerGuard};

/// Exhaustive test of the typed per-type `where:` filter on GraphQL
/// connection fields (rellinks / relcats), exercising negation, null,
/// inlined target columns, junction quantifiers, reverse membership,
/// and pagination + totalCount correlation.
#[test]
fn typed_where_filter_completeness() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // ── Fixture: create types ──
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE relcat (title TEXT, slug TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE relcat failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE rellink (url TEXT, category TEXT REFERENCES relcat)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE rellink failed: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    // ── Insert categories ──
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO relcat (title, slug) VALUES ('Rust', 'rust')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT Rust category failed: {r}");
    let cat_rust_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_rust_id.is_empty(), "cat_rust_id empty: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO relcat (title, slug) VALUES ('Python', 'py')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT Python category failed: {r}");
    let cat_py_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_py_id.is_empty(), "cat_py_id empty: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO relcat (title, slug) VALUES ('Empty', 'empty')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT Empty category failed: {r}");
    let cat_empty_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!cat_empty_id.is_empty(), "cat_empty_id empty: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    // ── Insert links ──
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO rellink (url, category) VALUES ('https://rust.example', '{cat_rust_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT rust link failed: {r}");
    let link1_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!link1_id.is_empty(), "link1_id empty: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO rellink (url, category) VALUES ('https://py.example', '{cat_py_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT py link failed: {r}");
    let link2_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!link2_id.is_empty(), "link2_id empty: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO rellink (title) VALUES ('orphan-link')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT orphan link failed: {r}");
    let link3_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .trim()
        .to_string();
    assert!(!link3_id.is_empty(), "link3_id empty: {r}");

    // ═══════════════════════════════════════════════════════════
    // ASSERTIONS
    // ═══════════════════════════════════════════════════════════

    // 1. NEGATION: notIn
    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ id: {{ notIn: ["{id}"] }} }}) {{ totalCount }} }}"#,
        id = link1_id
    ));
    assert!(r.get("errors").is_none(), "notIn query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "notIn totalCount should be 2: {r}"
    );

    // 1b. NEGATION: nin (exact alias of notIn)
    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ id: {{ nin: ["{id}"] }} }}) {{ totalCount }} }}"#,
        id = link1_id
    ));
    assert!(r.get("errors").is_none(), "nin query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "nin totalCount should be 2: {r}"
    );

    // 2. NULL: isNull true
    let r = server.graphql(r#"{ rellinks(where: { url: { isNull: true } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "isNull true query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "isNull true totalCount should be 1: {r}"
    );

    // 2b. NULL: isNull false
    let r = server.graphql(r#"{ rellinks(where: { url: { isNull: false } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "isNull false query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "isNull false totalCount should be 2: {r}"
    );

    // 3. FORWARD inlined col: category.title eq "Rust"
    let r = server.graphql(
        r#"{ rellinks(where: { category: { title: { eq: "Rust" } } }) { totalCount items { id url } } }"#,
    );
    assert!(r.get("errors").is_none(), "inlined title query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "inlined title totalCount should be 1: {r}"
    );
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "inlined title should return 1 item: {r}");
    assert_eq!(
        items[0]["url"].as_str().unwrap(),
        "https://rust.example",
        "inlined title url mismatch: {r}"
    );

    // 4. FORWARD inlined custom col: category.slug eq "py"
    let r = server.graphql(
        r#"{ rellinks(where: { category: { slug: { eq: "py" } } }) { totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "inlined slug query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "inlined slug totalCount should be 1: {r}"
    );

    // 5. FORWARD id-ops (back-compat): category eq cat_rust_id
    let r = server.graphql(&format!(
        r#"{{ rellinks(where: {{ category: {{ eq: "{id}" }} }}) {{ totalCount }} }}"#,
        id = cat_rust_id
    ));
    assert!(r.get("errors").is_none(), "category eq query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "category eq totalCount should be 1: {r}"
    );

    // 6. NONE: no relation (orphan link)
    let r = server.graphql(r#"{ rellinks(where: { category: { none: {} } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "none query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        1,
        "none totalCount should be 1: {r}"
    );

    // 6b. SOME: has relation
    let r = server.graphql(r#"{ rellinks(where: { category: { some: {} } }) { totalCount } }"#);
    assert!(r.get("errors").is_none(), "some query failed: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "some totalCount should be 2: {r}"
    );

    // 7. REVERSE some: relcats where rellinks has url containing "rust"
    let r = server.graphql(
        r#"{ relcats(where: { rellinks: { some: { url: { contains: "rust" } } } }) { totalCount items { id title } } }"#,
    );
    assert!(r.get("errors").is_none(), "reverse some query failed: {r}");
    assert_eq!(
        r["data"]["relcats"]["totalCount"].as_i64().unwrap(),
        1,
        "reverse some totalCount should be 1: {r}"
    );
    let items = r["data"]["relcats"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "reverse some should return 1 item: {r}");
    assert_eq!(
        items[0]["title"].as_str().unwrap(),
        "Rust",
        "reverse some title mismatch: {r}"
    );

    // 8. REVERSE none: relcats with no rellinks
    let r = server.graphql(r#"{ relcats(where: { rellinks: { none: {} } }) { totalCount items { id title } } }"#);
    assert!(r.get("errors").is_none(), "reverse none query failed: {r}");
    assert_eq!(
        r["data"]["relcats"]["totalCount"].as_i64().unwrap(),
        1,
        "reverse none totalCount should be 1: {r}"
    );
    let items = r["data"]["relcats"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "reverse none should return 1 item: {r}");
    assert_eq!(
        items[0]["title"].as_str().unwrap(),
        "Empty",
        "reverse none title mismatch: {r}"
    );

    // 9. PAGINATION + totalCount correlation
    // Page 0
    let r = server.graphql(
        r#"{ rellinks(where: { category: { some: {} } }, orderBy: { id: ASC }, limit: 1, offset: 0) { items { id url } totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "pagination page 0 failed: {r}");
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "page 0 should have 1 item: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "page 0 totalCount should be 2: {r}"
    );
    let p0_url = items[0]["url"].as_str().unwrap().to_string();

    // Page 1
    let r = server.graphql(
        r#"{ rellinks(where: { category: { some: {} } }, orderBy: { id: ASC }, limit: 1, offset: 1) { items { id url } totalCount } }"#,
    );
    assert!(r.get("errors").is_none(), "pagination page 1 failed: {r}");
    let items = r["data"]["rellinks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "page 1 should have 1 item: {r}");
    assert_eq!(
        r["data"]["rellinks"]["totalCount"].as_i64().unwrap(),
        2,
        "page 1 totalCount should be 2: {r}"
    );
    let p1_url = items[0]["url"].as_str().unwrap().to_string();

    // No duplicates across pages
    assert_ne!(p0_url, p1_url, "page 0 and page 1 should have different URLs: {r}");
}
