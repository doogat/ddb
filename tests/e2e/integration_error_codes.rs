//! Port of tests/integration.sh:2109-2157 (PRD 00131 structured-error code
//! propagation): GraphQL errors for constraint violations carry a
//! machine-readable `extensions.code` (and related fields), not just a
//! human-readable message.

use crate::common::{DdbTestRepo, ServerGuard};

const CREATE_PUV_LINK: &str = "CREATE TABLE puv_link (title VARCHAR(255), slug VARCHAR(255) NOT NULL, space VARCHAR(255) NOT NULL, UNIQUE(slug, space))";

/// Fresh repo + server with the `puv_link` typedef (UNIQUE(slug, space))
/// already created via `executeSql`. Each test gets its own instance.
fn setup_puv_link() -> (DdbTestRepo, ServerGuard) {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": CREATE_PUV_LINK }),
    );
    assert!(
        result.get("errors").is_none(),
        "CREATE TABLE puv_link failed: {result}"
    );
    (repo, server)
}

#[test]
fn integration_49_1_unique_violation_extensions_code() {
    let (_repo, server) = setup_puv_link();

    let first = server.graphql(
        r#"mutation { createDoogat(input: {type: "puv_link", title: "first", fields: "{\"slug\":\"hn\",\"space\":\"news\"}"}) { id } }"#,
    );
    assert!(
        first.get("errors").is_none(),
        "first create should succeed: {first}"
    );

    let dup = server.graphql(
        r#"mutation { createDoogat(input: {type: "puv_link", title: "dup", fields: "{\"slug\":\"hn\",\"space\":\"news\"}"}) { id } }"#,
    );
    assert!(
        dup.get("errors").is_some(),
        "duplicate slug+space should error: {dup}"
    );
    assert_eq!(
        dup["errors"][0]["extensions"]["code"].as_str(),
        Some("UNIQUE_VIOLATION"),
        "unexpected extensions.code: {dup}"
    );
    assert_eq!(
        dup["errors"][0]["extensions"]["columns"],
        serde_json::json!(["slug", "space"]),
        "unexpected extensions.columns: {dup}"
    );
    let values = dup["errors"][0]["extensions"]["values"]
        .as_array()
        .unwrap_or_else(|| panic!("extensions.values should be an array: {dup}"));
    assert_eq!(
        values.len(),
        2,
        "expected 2 colliding values in extensions.values: {dup}"
    );
}

#[test]
fn integration_49_2_not_null_violation_extensions_code() {
    let (_repo, server) = setup_puv_link();

    let result = server.graphql(
        r#"mutation { createDoogat(input: {type: "puv_link", title: "missing-slug", fields: "{\"space\":\"news\"}"}) { id } }"#,
    );
    assert!(
        result.get("errors").is_some(),
        "missing required slug field should error: {result}"
    );
    assert_eq!(
        result["errors"][0]["extensions"]["code"].as_str(),
        Some("NOT_NULL_VIOLATION"),
        "unexpected extensions.code: {result}"
    );
    assert_eq!(
        result["errors"][0]["extensions"]["column"].as_str(),
        Some("slug"),
        "unexpected extensions.column: {result}"
    );
}

#[test]
fn integration_49_3_createmany_error_extensions_code() {
    let (_repo, server) = setup_puv_link();

    let seed = server.graphql(
        r#"mutation { createDoogat(input: {type: "puv_link", title: "first", fields: "{\"slug\":\"hn\",\"space\":\"news\"}"}) { id } }"#,
    );
    assert!(seed.get("errors").is_none(), "seed create failed: {seed}");

    let result = server.graphql(
        r#"mutation { createMany(
            inputs: [{type: "puv_link", title: "cm-dup", fields: "{\"slug\":\"hn\",\"space\":\"news\"}"}],
            onConflict: ERROR
        ) { id } }"#,
    );
    assert!(
        result.get("errors").is_some(),
        "createMany colliding with seeded row should error: {result}"
    );
    assert_eq!(
        result["errors"][0]["extensions"]["code"].as_str(),
        Some("UNIQUE_VIOLATION"),
        "unexpected extensions.code: {result}"
    );
}

#[test]
fn integration_49_4_createmany_intra_batch_error_extensions_code() {
    let (_repo, server) = setup_puv_link();

    let result = server.graphql(
        r#"mutation { createMany(
            inputs: [
                {type: "puv_link", title: "cm-a", fields: "{\"slug\":\"twin\",\"space\":\"news\"}"},
                {type: "puv_link", title: "cm-b", fields: "{\"slug\":\"twin\",\"space\":\"news\"}"}
            ],
            onConflict: ERROR
        ) { id } }"#,
    );
    assert!(
        result.get("errors").is_some(),
        "two inputs colliding within the same batch should error: {result}"
    );
    assert_eq!(
        result["errors"][0]["extensions"]["code"].as_str(),
        Some("UNIQUE_VIOLATION"),
        "unexpected extensions.code: {result}"
    );
}
