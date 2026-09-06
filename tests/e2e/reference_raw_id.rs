use crate::common::{DdbTestRepo, ServerGuard};

/// Helper: create category and bookmark types, insert a category, a bookmark,
/// and link them via the junction table. Returns (bookmark_id, category_id).
fn setup_linked_bookmark_category(server: &ServerGuard) -> (String, String) {
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

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO category (label) VALUES ('tech')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT category failed: {r}");
    let cat_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT bookmark failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

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
fn raw_id_field_available_for_references_column() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let (_bm_id, cat_id) = setup_linked_bookmark_category(&server);

    // category_id should return the raw doogat ID as a scalar String
    let r = server.graphql(r#"{ bookmarks { items { category_id } } }"#);
    assert!(r.get("errors").is_none(), "category_id query failed: {r}");
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "expected 1 bookmark: {r}");
    let raw_id = items[0]["category_id"]
        .as_str()
        .expect("category_id should be a non-null String");
    assert_eq!(
        raw_id, cat_id,
        "category_id should match the category's doogat ID"
    );
}

#[test]
fn raw_id_field_null_when_no_reference() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create types but don't link
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

    // Insert a bookmark without linking to any category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT bookmark failed: {r}");

    let r = server.graphql(r#"{ bookmarks { items { category_id } } }"#);
    assert!(
        r.get("errors").is_none(),
        "category_id null query failed: {r}"
    );
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(
        items[0]["category_id"].is_null(),
        "category_id should be null when no reference exists: {r}"
    );
}

#[test]
fn raw_id_coexists_with_object_resolver() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let (_bm_id, cat_id) = setup_linked_bookmark_category(&server);

    // Query both the scalar raw ID and the resolved object in the same query
    let r = server.graphql(r#"{ bookmarks { items { category_id category { id label } } } }"#);
    assert!(r.get("errors").is_none(), "coexistence query failed: {r}");
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);

    let raw_id = items[0]["category_id"]
        .as_str()
        .expect("category_id should be a non-null String");
    let resolved_id = items[0]["category"]["id"]
        .as_str()
        .expect("category.id should be a non-null String");
    let label = items[0]["category"]["label"]
        .as_str()
        .expect("category.label should be a non-null String");

    assert_eq!(
        raw_id, cat_id,
        "category_id scalar should match category ID"
    );
    assert_eq!(resolved_id, cat_id, "category.id should match category ID");
    assert_eq!(
        raw_id, resolved_id,
        "category_id and category.id must be identical"
    );
    assert_eq!(label, "tech");
}

#[test]
fn raw_id_field_pins_first_linked_with_multiple_junction_rows() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

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

    // Three categories, minted in ascending id order.
    let mut category_ids = Vec::new();
    for label in ["alpha", "beta", "gamma"] {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": format!("INSERT INTO category (label) VALUES ('{label}')") }),
        );
        assert!(
            r.get("errors").is_none(),
            "INSERT category {label} failed: {r}"
        );
        category_ids.push(
            r["data"]["executeSql"]["message"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO bookmark (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT bookmark failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Link the categories out of id order (gamma, alpha, beta) so a resolver
    // that picked the numerically smallest category id would give a
    // different answer than one that pins the first-*linked* junction row.
    for cat_id in [&category_ids[2], &category_ids[0], &category_ids[1]] {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({
                "sql": format!(
                    "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
                )
            }),
        );
        assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");
    }

    let r = server.graphql(r#"{ bookmarks { items { category_id } } }"#);
    assert!(r.get("errors").is_none(), "category_id query failed: {r}");
    let items = r["data"]["bookmarks"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "expected 1 bookmark: {r}");
    let raw_id = items[0]["category_id"]
        .as_str()
        .expect("category_id should be a non-null String");
    assert_eq!(
        raw_id, category_ids[2],
        "category_id must pin the first-linked category (gamma), not the numerically smallest one"
    );
}

#[test]
fn id_suffix_column_exposes_scalar_and_object() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create link type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE link (title TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE link failed: {r}");

    // Create note type with _id suffix column referencing link
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE note (content TEXT, link_id TEXT REFERENCES link)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE note failed: {r}");

    // Insert a link
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO link (title) VALUES ('Example Link')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT link failed: {r}");
    let link_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Insert a note
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO note (content) VALUES ('my note')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT note failed: {r}");
    let note_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Junction table for column "link_id" on table "note" is "note_link_id"
    // with columns "note_id" and "link_id_id"
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!(
                "INSERT INTO note_link_id (note_id, link_id_id) VALUES ('{note_id}', '{link_id}')"
            )
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");

    // For column "link_id" with _id suffix:
    // - link_id → raw scalar (original column name, now returns String)
    // - link → resolved object (stripped _id suffix)
    let r = server.graphql(r#"{ notes { items { link_id link { id title } } } }"#);
    assert!(r.get("errors").is_none(), "id_suffix query failed: {r}");
    let items = r["data"]["notes"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "expected 1 note: {r}");

    let scalar_id = items[0]["link_id"]
        .as_str()
        .expect("link_id should be a non-null String");
    let resolved_id = items[0]["link"]["id"]
        .as_str()
        .expect("link.id should be a non-null String");
    let title = items[0]["link"]["title"]
        .as_str()
        .expect("link.title should be a non-null String");

    assert_eq!(scalar_id, link_id, "link_id scalar should match link ID");
    assert_eq!(resolved_id, link_id, "link.id should match link ID");
    assert_eq!(
        scalar_id, resolved_id,
        "link_id and link.id must be identical"
    );
    assert_eq!(title, "Example Link");
}

#[test]
fn plural_resolver_with_limit() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create lcat type and lbm type with REFERENCES
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE lcat (label TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE lcat failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE lbm (url TEXT, lcat TEXT REFERENCES lcat)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE lbm failed: {r}");

    // Insert 3 categories
    let mut cat_ids = Vec::new();
    for label in ["alpha", "beta", "gamma"] {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": format!("INSERT INTO lcat (label) VALUES ('{label}')") }),
        );
        assert!(
            r.get("errors").is_none(),
            "INSERT lcat '{label}' failed: {r}"
        );
        cat_ids.push(
            r["data"]["executeSql"]["message"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Insert a bookmark
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO lbm (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT lbm failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Link all 3 categories to the bookmark
    for cat_id in &cat_ids {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({
                "sql": format!(
                    "INSERT INTO lbm_lcat (lbm_id, lcat_id) VALUES ('{bm_id}', '{cat_id}')"
                )
            }),
        );
        assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");
    }

    // Query with limit: 2
    let r = server.graphql(r#"{ lbms { items { lcats(limit: 2) { id label } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "plural resolver limit query failed: {r}"
    );
    let items = r["data"]["lbms"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let cats = items[0]["lcats"].as_array().unwrap();
    assert_eq!(
        cats.len(),
        2,
        "expected exactly 2 categories with limit: 2, got {}: {r}",
        cats.len()
    );
}

#[test]
fn plural_resolver_with_order_by() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create ocat type and obm type with REFERENCES
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE ocat (label TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE ocat failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE obm (url TEXT, ocat TEXT REFERENCES ocat)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE obm failed: {r}");

    // Insert 3 categories in non-alphabetical order
    let mut cat_ids = Vec::new();
    for label in ["cherry", "apple", "banana"] {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": format!("INSERT INTO ocat (label) VALUES ('{label}')") }),
        );
        assert!(
            r.get("errors").is_none(),
            "INSERT ocat '{label}' failed: {r}"
        );
        cat_ids.push(
            r["data"]["executeSql"]["message"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Insert a bookmark
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO obm (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT obm failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Link all 3 categories
    for cat_id in &cat_ids {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({
                "sql": format!(
                    "INSERT INTO obm_ocat (obm_id, ocat_id) VALUES ('{bm_id}', '{cat_id}')"
                )
            }),
        );
        assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");
    }

    // Query with orderBy: "label" (ascending by default)
    let r = server.graphql(r#"{ obms { items { ocats(orderBy: "label") { label } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "plural resolver orderBy query failed: {r}"
    );
    let items = r["data"]["obms"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let cats = items[0]["ocats"].as_array().unwrap();
    assert_eq!(cats.len(), 3, "expected 3 categories: {r}");
    let labels: Vec<&str> = cats.iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec!["apple", "banana", "cherry"],
        "expected alphabetical order: {r}"
    );
}

#[test]
fn plural_resolver_with_order_by_desc() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create dcat type and dbm type with REFERENCES
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE dcat (label TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE dcat failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE dbm (url TEXT, dcat TEXT REFERENCES dcat)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE dbm failed: {r}");

    // Insert 3 categories in non-alphabetical order
    let mut cat_ids = Vec::new();
    for label in ["cherry", "apple", "banana"] {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": format!("INSERT INTO dcat (label) VALUES ('{label}')") }),
        );
        assert!(
            r.get("errors").is_none(),
            "INSERT dcat '{label}' failed: {r}"
        );
        cat_ids.push(
            r["data"]["executeSql"]["message"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Insert a bookmark
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO dbm (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT dbm failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Link all 3 categories
    for cat_id in &cat_ids {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({
                "sql": format!(
                    "INSERT INTO dbm_dcat (dbm_id, dcat_id) VALUES ('{bm_id}', '{cat_id}')"
                )
            }),
        );
        assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");
    }

    // Query with orderBy: "label", orderDir: "DESC"
    let r = server
        .graphql(r#"{ dbms { items { dcats(orderBy: "label", orderDir: "DESC") { label } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "plural resolver orderBy DESC query failed: {r}"
    );
    let items = r["data"]["dbms"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let cats = items[0]["dcats"].as_array().unwrap();
    assert_eq!(cats.len(), 3, "expected 3 categories: {r}");
    let labels: Vec<&str> = cats.iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec!["cherry", "banana", "apple"],
        "expected reverse alphabetical order: {r}"
    );
}

#[test]
fn plural_resolver_with_order_by_and_limit() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create olcat type and olbm type with REFERENCES
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE olcat (label TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE olcat failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE olbm (url TEXT, olcat TEXT REFERENCES olcat)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE olbm failed: {r}");

    // Insert 3 categories in non-alphabetical order
    let mut cat_ids = Vec::new();
    for label in ["cherry", "apple", "banana"] {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": format!("INSERT INTO olcat (label) VALUES ('{label}')") }),
        );
        assert!(
            r.get("errors").is_none(),
            "INSERT olcat '{label}' failed: {r}"
        );
        cat_ids.push(
            r["data"]["executeSql"]["message"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Insert a bookmark
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO olbm (url) VALUES ('https://example.com')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT olbm failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    // Link all 3 categories
    for cat_id in &cat_ids {
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({
                "sql": format!(
                    "INSERT INTO olbm_olcat (olbm_id, olcat_id) VALUES ('{bm_id}', '{cat_id}')"
                )
            }),
        );
        assert!(r.get("errors").is_none(), "INSERT junction failed: {r}");
    }

    // Query with orderBy: "label" and limit: 2
    let r =
        server.graphql(r#"{ olbms { items { olcats(orderBy: "label", limit: 2) { label } } } }"#);
    assert!(
        r.get("errors").is_none(),
        "plural resolver orderBy + limit query failed: {r}"
    );
    let items = r["data"]["olbms"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let cats = items[0]["olcats"].as_array().unwrap();
    assert_eq!(
        cats.len(),
        2,
        "expected 2 categories with limit: 2, got {}: {r}",
        cats.len()
    );
    let labels: Vec<&str> = cats.iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec!["apple", "banana"],
        "expected first 2 alphabetically: {r}"
    );

    let r = server.graphql(
        r#"{ olbms { items { olcats(orderBy: "label", orderDir: "DESC", limit: 2) { label } } } }"#,
    );
    assert!(r.get("errors").is_none(), "DESC + limit failed: {r}");
    let cats = r["data"]["olbms"]["items"][0]["olcats"].as_array().unwrap();
    let labels: Vec<&str> = cats.iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        ["cherry", "banana"],
        "DESC must order before truncating: {r}"
    );
}
