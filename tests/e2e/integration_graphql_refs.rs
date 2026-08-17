use crate::common::{DdbTestRepo, ServerGuard};

#[test]
fn integration_38b_rest_structured_references_object() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create category type
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE mvcategory (name TEXT)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE mvcategory failed: {r}");

    // Create bookmark type with REFERENCES
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE mvbookmark (mvcategory TEXT REFERENCES mvcategory)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE mvbookmark failed: {r}");

    // Insert a category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO mvcategory (name) VALUES ('Science')" }),
    );
    assert!(r.get("errors").is_none(), "INSERT mvcategory failed: {r}");
    let cat_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!cat_id.is_empty(), "category ID empty");

    std::thread::sleep(std::time::Duration::from_secs(1));

    // Insert a bookmark linked to the category
    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({
            "sql": format!("INSERT INTO mvbookmark (mvcategory) VALUES ('{cat_id}')")
        }),
    );
    assert!(r.get("errors").is_none(), "INSERT mvbookmark failed: {r}");
    let bm_id = r["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!bm_id.is_empty(), "bookmark ID empty");

    // REST GET the bookmark and inspect the references envelope
    let resp = server.rest_get(&format!("/doogats/{bm_id}"));
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let refs = body["data"]["references"]["mvcategory"]
        .as_array()
        .expect("references.mvcategory should be an array");
    let ref_ids: Vec<&str> = refs.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        ref_ids.contains(&cat_id.as_str()),
        "expected mvcategory references to contain {cat_id}, got: {ref_ids:?}"
    );
}

#[test]
fn integration_38c2_boolean_coercion() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE smokepin (pinned BOOLEAN)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE smokepin failed: {r}");

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO smokepin (title, pinned) VALUES ('PinTest', true)" }),
    );
    assert!(r.get("errors").is_none(), "INSERT PinTest failed: {r}");

    std::thread::sleep(std::time::Duration::from_secs(1));

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "INSERT INTO smokepin (title, pinned) VALUES ('FalseTest', false)" }),
    );
    assert!(r.get("errors").is_none(), "INSERT FalseTest failed: {r}");

    let result = server
        .graphql(r#"{ sql(query: "SELECT pinned FROM smokepin WHERE pinned = 1") { rows } }"#);
    assert!(
        result.get("errors").is_none(),
        "pinned=1 query failed: {result}"
    );
    let rows = result["data"]["sql"]["rows"].as_array().unwrap();
    assert!(
        rows.iter().any(|row| row.as_str().unwrap().contains("true")),
        "expected a row containing \"true\": {rows:?}"
    );

    let result = server
        .graphql(r#"{ sql(query: "SELECT pinned FROM smokepin WHERE pinned = 0") { rows } }"#);
    assert!(
        result.get("errors").is_none(),
        "pinned=0 query failed: {result}"
    );
    let rows = result["data"]["sql"]["rows"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|row| row.as_str().unwrap().contains("false")),
        "expected a row containing \"false\": {rows:?}"
    );
}

#[test]
fn integration_38d_distinct_typed_query() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE foo (bar TEXT, baz INTEGER)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE foo failed: {r}");

    for (bar, baz) in [("val", 1), ("val", 2), ("other", 3)] {
        let sql = format!("INSERT INTO foo (bar, baz) VALUES ('{bar}', {baz})");
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": sql }),
        );
        assert!(r.get("errors").is_none(), "INSERT failed: {r}");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let result = server.graphql(r#"{ foos(distinct: "bar") { items { bar } totalCount } }"#);
    assert!(
        result.get("errors").is_none(),
        "distinct query failed: {result}"
    );
    assert_eq!(
        result["data"]["foos"]["totalCount"].as_i64().unwrap(),
        2,
        "distinct should dedup by bar: {result}"
    );

    let result =
        server.graphql(r#"{ foos(distinct: "bar", where: { baz: { gte: 2 } }) { totalCount } }"#);
    assert!(
        result.get("errors").is_none(),
        "distinct+where query failed: {result}"
    );
    assert_eq!(
        result["data"]["foos"]["totalCount"].as_i64().unwrap(),
        2,
        "distinct+where should match ('val',2) and ('other',3): {result}"
    );
}

#[test]
fn integration_38e_groupby_aggregate() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let r = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE foo (bar TEXT, baz INTEGER)" }),
    );
    assert!(r.get("errors").is_none(), "CREATE foo failed: {r}");

    for (bar, baz) in [("val", 1), ("val", 2), ("other", 3)] {
        let sql = format!("INSERT INTO foo (bar, baz) VALUES ('{bar}', {baz})");
        let r = server.graphql_with_vars(
            r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
            serde_json::json!({ "sql": sql }),
        );
        assert!(r.get("errors").is_none(), "INSERT failed: {r}");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let result = server.graphql(r#"{ foosAggregate(groupBy: "bar") { groups { key count } } }"#);
    assert!(
        result.get("errors").is_none(),
        "groupBy aggregate failed: {result}"
    );
    let groups = result["data"]["foosAggregate"]["groups"]
        .as_array()
        .unwrap();
    let val_group = groups
        .iter()
        .find(|g| g["key"].as_str().unwrap() == "val")
        .expect("expected a group with key 'val'");
    assert_eq!(val_group["count"].as_i64().unwrap(), 2);
    let other_group = groups
        .iter()
        .find(|g| g["key"].as_str().unwrap() == "other")
        .expect("expected a group with key 'other'");
    assert_eq!(other_group["count"].as_i64().unwrap(), 1);

    let result = server
        .graphql(r#"{ foosAggregate(groupBy: "bar") { groups { key count minBaz maxBaz } } }"#);
    assert!(
        result.get("errors").is_none(),
        "groupBy aggregate with min/max failed: {result}"
    );
    let groups = result["data"]["foosAggregate"]["groups"]
        .as_array()
        .unwrap();
    for group in groups {
        assert!(
            group.get("minBaz").is_some(),
            "minBaz missing on group: {group}"
        );
        assert!(
            group.get("maxBaz").is_some(),
            "maxBaz missing on group: {group}"
        );
    }

    let result = server.graphql(
        r#"{ foosAggregate(groupBy: "bar", where: { baz: { gte: 2 } }) { groups { key count } } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "groupBy+where aggregate failed: {result}"
    );
    let groups = result["data"]["foosAggregate"]["groups"]
        .as_array()
        .unwrap();
    assert!(!groups.is_empty(), "expected at least one group: {result}");

    let result = server.graphql(r#"{ foosAggregate { count } }"#);
    assert!(
        result.get("errors").is_none(),
        "non-grouped aggregate failed: {result}"
    );
    assert_eq!(
        result["data"]["foosAggregate"]["count"].as_i64().unwrap(),
        3
    );
}
