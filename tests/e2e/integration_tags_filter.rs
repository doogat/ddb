use crate::common::{DdbTestRepo, ServerGuard};

#[test]
fn integration_45_g11_tags_filter_nullable_operators() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE g11link (title TEXT, url VARCHAR(255))") { message } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE g11link failed: {create}"
    );

    let created = server.graphql(
        r#"mutation { createDoogat(input: {title: "Tagged", type: "g11link", tags: ["rust", "sql"], fields: "{\"url\":\"https://example.com\"}"}) { id } }"#,
    );
    assert!(
        created.get("errors").is_none(),
        "createDoogat with tags failed: {created}"
    );

    let contains = server.graphql(
        r#"{ g11links(where: {tags: {contains: "rust"}}) { totalCount items { id } } }"#,
    );
    assert!(
        contains.get("errors").is_none(),
        "contains filter failed: {contains}"
    );
    assert_eq!(
        contains["data"]["g11links"]["totalCount"],
        serde_json::json!(1)
    );

    let contains_all = server.graphql(
        r#"{ g11links(where: {tags: {containsAll: ["rust", "sql"]}}) { totalCount } }"#,
    );
    assert!(
        contains_all.get("errors").is_none(),
        "containsAll filter failed: {contains_all}"
    );
    assert_eq!(
        contains_all["data"]["g11links"]["totalCount"],
        serde_json::json!(1)
    );

    let contains_any = server.graphql(
        r#"{ g11links(where: {tags: {containsAny: ["rust", "go"]}}) { totalCount } }"#,
    );
    assert!(
        contains_any.get("errors").is_none(),
        "containsAny filter failed: {contains_any}"
    );
    assert_eq!(
        contains_any["data"]["g11links"]["totalCount"],
        serde_json::json!(1)
    );

    let empty_filter = server.graphql(r#"{ g11links(where: {tags: {}}) { totalCount } }"#);
    assert!(
        empty_filter.get("errors").is_some(),
        "an empty tags filter object should error: {empty_filter}"
    );
    assert!(
        empty_filter["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("tags filter requires at least one of"),
        "error message mismatch: {empty_filter}"
    );

    let empty_contains_all =
        server.graphql(r#"{ g11links(where: {tags: {containsAll: []}}) { totalCount } }"#);
    assert!(
        empty_contains_all.get("errors").is_some(),
        "an empty containsAll should error: {empty_contains_all}"
    );
    assert!(
        empty_contains_all["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("containsAll cannot be empty"),
        "error message mismatch: {empty_contains_all}"
    );

    let empty_contains_any =
        server.graphql(r#"{ g11links(where: {tags: {containsAny: []}}) { totalCount } }"#);
    assert!(
        empty_contains_any.get("errors").is_some(),
        "an empty containsAny should error: {empty_contains_any}"
    );
    assert!(
        empty_contains_any["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("containsAny cannot be empty"),
        "error message mismatch: {empty_contains_any}"
    );

    let introspect = server.graphql(
        r#"{ __type(name: "TagsFilter") { inputFields { name type { kind ofType { kind name ofType { kind name } } } } } }"#,
    );
    assert!(
        introspect.get("errors").is_none(),
        "introspection failed: {introspect}"
    );
    let fields = introspect["data"]["__type"]["inputFields"]
        .as_array()
        .unwrap();
    for field_name in ["containsAll", "containsAny"] {
        let field = fields
            .iter()
            .find(|f| f["name"].as_str() == Some(field_name))
            .unwrap_or_else(|| panic!("{field_name} not found in TagsFilter: {introspect}"));
        assert_eq!(
            field["type"]["kind"].as_str(),
            Some("LIST"),
            "{field_name} should be a nullable list (LIST, not NON_NULL): {introspect}"
        );
    }
}
