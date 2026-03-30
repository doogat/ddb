use crate::common::{DdbTestRepo, ServerGuard};

/// Create tables, link via junction, and verify GraphQL resolves the relation.
fn setup_bookmark_category(server: &ServerGuard) -> (String, String) {
    // Create category type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE category (label TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE category failed: {r}");

    // Create bookmark type with REFERENCES
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE bookmark failed: {r}");

    // Insert a category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO category (label) VALUES ('tech')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT category failed: {r}");
    let cat_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!cat_id.is_empty(), "category ID empty");

    // Wait for unique timestamp
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Insert a bookmark
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT bookmark failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!bm_id.is_empty(), "bookmark ID empty");

    // Link bookmark to category via junction table
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");

    (bm_id, cat_id)
}

#[test]
fn singular_reference_resolves_as_typed_object() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let (_bm_id, cat_id) = setup_bookmark_category(&server);

    // Query the bookmark's singular category field - should resolve as Category object
    let r = server.graphql(r#"{ bookmarks { items { id category { id title label } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "singular relation query failed: {r}"
    );
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "expected 1 bookmark: {r}");
    let cat = &items[0]["category"];
    assert!(!cat.is_null(), "category should not be null: {r}");
    assert_eq!(cat["id"].as_str().unwrap(), cat_id);
    assert_eq!(cat["label"].as_str().unwrap(), "tech");
}

#[test]
fn plural_reference_resolves_as_typed_object_list() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let (_bm_id, cat_id) = setup_bookmark_category(&server);

    // Query the bookmark's plural categories field
    let r = server.graphql(r#"{ bookmarks { items { id categories { id title label } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "plural relation query failed: {r}"
    );
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let cats = items[0]["categories"].as_array().unwrap();
    assert_eq!(cats.len(), 1, "expected 1 category in list: {r}");
    assert_eq!(cats[0]["id"].as_str().unwrap(), cat_id);
    assert_eq!(cats[0]["label"].as_str().unwrap(), "tech");
}

#[test]
fn null_reference_returns_null() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create types but don't link them
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE category (label TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE category failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE bookmark failed: {r}");

    // Insert a bookmark WITHOUT linking to a category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT bookmark failed: {r}");

    // Singular should be null, plural should be empty list
    let r = server.graphql(
        r#"{ bookmarks { items { id category { id } categories { id } } } }"#,
    );
    assert!(
        r.get("errors").is_none(),
        "null reference query failed: {r}"
    );
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0]["category"].is_null(), "category should be null: {r}");
    let cats = items[0]["categories"].as_array().unwrap();
    assert!(cats.is_empty(), "categories should be empty: {r}");
}

#[test]
fn reference_to_missing_doogat_returns_null() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create types
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE category (label TEXT)" }),
    );
    assert!(r.get("errors").is_none());

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)" }),
    );
    assert!(r.get("errors").is_none());

    // Insert category, then bookmark linked to it
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO category (label) VALUES ('temp')" }),
    );
    let cat_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://example.com')" }),
    );
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Link
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none());

    // Delete the category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!("DELETE FROM category WHERE id = '{cat_id}'")
        }),
    );
    assert!(r.get("errors").is_none(), "DELETE category failed: {r}");

    // The reference still exists in the bookmark's reference section,
    // but the target doogat is gone - should return null
    let r = server.graphql(r#"{ bookmarks { items { category { id } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "missing target query failed: {r}"
    );
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(
        items[0]["category"].is_null(),
        "category should be null after deletion: {r}"
    );
}

#[test]
fn cross_type_resolution_includes_typed_fields() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a "project" type with custom columns
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE project (name TEXT, priority INTEGER)" }),
    );
    assert!(r.get("errors").is_none());

    // Create a "task" type referencing project
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE task (description TEXT, project TEXT REFERENCES project)" }),
    );
    assert!(r.get("errors").is_none());

    // Insert a project with priority
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO project (name, priority) VALUES ('Alpha', 1)" }),
    );
    let proj_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Insert a task
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO task (description) VALUES ('do stuff')" }),
    );
    let task_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Link task to project
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO task_project (task_id, project_id) VALUES ('{task_id}', '{proj_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "junction insert failed: {r}");

    // Query task's project with typed fields (name, priority)
    let r = server.graphql(r#"{ tasks { items { id project { id name priority } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "cross-type resolution failed: {r}"
    );
    let items = r["data"]["tasks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let proj = &items[0]["project"];
    assert!(!proj.is_null(), "project should resolve: {r}");
    assert_eq!(proj["id"].as_str().unwrap(), proj_id);
    assert_eq!(proj["name"].as_str().unwrap(), "Alpha");
    assert_eq!(proj["priority"].as_i64().unwrap(), 1);
}

#[test]
fn typed_connection_includes_tags() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a type and insert doogats with tags
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE article (topic TEXT)" }),
    );
    assert!(r.get("errors").is_none());

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO article (topic) VALUES ('rust')" }),
    );
    assert!(r.get("errors").is_none());
    let id1 = r["data"]["executeSql"]["message"].as_str().unwrap().to_string();

    // Tag the doogat
    let r = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id tags } }"#,
        serde_json::json!({ "input": { "id": id1, "tags": ["coding", "systems"] } }),
    );
    assert!(r.get("errors").is_none(), "tag update failed: {r}");

    // Query via typed connection - tags should be present
    let r = server.graphql(r#"{ articles { items { id topic tags } } }"#);
    assert!(r.get("errors").is_none(), "tags on typed connection failed: {r}");
    let items = r["data"]["articles"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let tags: Vec<&str> = items[0]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(tags.contains(&"coding"), "missing 'coding' tag: {r}");
    assert!(tags.contains(&"systems"), "missing 'systems' tag: {r}");
}

#[test]
fn batch_plural_references_multiple_items() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create category type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE category (label TEXT)" }),
    );
    assert!(r.get("errors").is_none());

    // Create bookmark type with REFERENCES
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)" }),
    );
    assert!(r.get("errors").is_none());

    // Insert 2 categories
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO category (label) VALUES ('tech')" }),
    );
    let cat1 = r["data"]["executeSql"]["message"].as_str().unwrap().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO category (label) VALUES ('science')" }),
    );
    let cat2 = r["data"]["executeSql"]["message"].as_str().unwrap().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Insert 2 bookmarks
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://a.com')" }),
    );
    let bm1 = r["data"]["executeSql"]["message"].as_str().unwrap().to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://b.com')" }),
    );
    let bm2 = r["data"]["executeSql"]["message"].as_str().unwrap().to_string();

    // Link bm1 to both categories, bm2 to none
    for cat in [&cat1, &cat2] {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({
                "sql": format!(
                    "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm1}', '{cat}')"
                )
            }),
        );
        assert!(r.get("errors").is_none(), "junction insert failed: {r}");
    }

    // Query all bookmarks with categories
    let r = server.graphql(r#"{ bookmarks { items { id url categories { id label } } } }"#);
    assert!(r.get("errors").is_none(), "batch plural query failed: {r}");
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "expected 2 bookmarks: {r}");

    // Find each bookmark's categories
    let bm1_item = items.iter().find(|i| i["id"].as_str().unwrap() == bm1).unwrap();
    let bm2_item = items.iter().find(|i| i["id"].as_str().unwrap() == bm2).unwrap();

    let bm1_cats = bm1_item["categories"].as_array().unwrap();
    assert_eq!(bm1_cats.len(), 2, "bm1 should have 2 categories: {r}");
    let labels: Vec<&str> = bm1_cats.iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"tech"));
    assert!(labels.contains(&"science"));

    let bm2_cats = bm2_item["categories"].as_array().unwrap();
    assert!(bm2_cats.is_empty(), "bm2 should have no categories: {r}");
}
