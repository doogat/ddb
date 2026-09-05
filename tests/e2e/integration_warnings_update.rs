//! Port of tests/integration.sh:2538-2657 (PRD 00147 REST warnings envelope,
//! PRD 00154 GraphQL warnings envelope, PRD 00149 update warning/error shape).

use crate::common::{DdbTestRepo, ServerGuard};

const CREATE_IG_WARN_DEMO: &str = "CREATE TABLE ig_warn_demo (note TEXT)";
const SET_IG_WARN_DEMO_TITLE_TEMPLATE: &str =
    "ALTER TABLE ig_warn_demo SET TITLE TEMPLATE 'auto-warn'";
const CREATE_IG_UPD_DEMO: &str = "CREATE TABLE ig_upd_demo (note TEXT)";

/// Fresh repo + server with the `ig_warn_demo` typedef (title_template
/// `'auto-warn'`) already created via the CLI. Each test gets its own
/// instance.
fn setup_ig_warn_demo() -> (DdbTestRepo, ServerGuard) {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", CREATE_IG_WARN_DEMO])
        .assert()
        .success()
        .stdout(predicates::str::contains("table ig_warn_demo created"));
    repo.ddb()
        .args(["query", SET_IG_WARN_DEMO_TITLE_TEMPLATE])
        .assert()
        .success()
        .stdout(predicates::str::contains("title template"));
    let server = ServerGuard::start(&repo);
    (repo, server)
}

/// Fresh repo + server with the `ig_upd_demo` typedef (no title_template)
/// already created via `executeSql`. Each test gets its own instance.
fn setup_ig_upd_demo() -> (DdbTestRepo, ServerGuard) {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let create = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": CREATE_IG_UPD_DEMO }),
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE ig_upd_demo failed: {create}"
    );
    (repo, server)
}

#[test]
fn integration_56_a_rest_create_titled_warnings_empty() {
    let (_repo, server) = setup_ig_warn_demo();

    let response = server.rest_post(
        "/doogats",
        serde_json::json!({ "title": "explicit", "type": "ig_warn_demo" }),
    );
    assert_eq!(response.status().as_u16(), 201);
    let body: serde_json::Value = response.json().expect("invalid json");
    assert_eq!(
        body["warnings"],
        serde_json::json!([]),
        "expected empty warnings array: {body}"
    );
}

#[test]
fn integration_56_b_rest_create_omitted_title_title_from_template_warning() {
    let (_repo, server) = setup_ig_warn_demo();

    let response = server.rest_post("/doogats", serde_json::json!({ "type": "ig_warn_demo" }));
    assert_eq!(response.status().as_u16(), 201);
    let body: serde_json::Value = response.json().expect("invalid json");
    let warnings = body["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("warnings should be an array: {body}"));
    assert_eq!(warnings.len(), 1, "expected exactly 1 warning: {body}");
    assert_eq!(
        warnings[0]["code"].as_str(),
        Some("TITLE_FROM_TEMPLATE"),
        "unexpected warning code: {body}"
    );
    let message = warnings[0]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("warning message should be a string: {body}"));
    assert!(
        message.contains("title_template"),
        "warning message should mention title_template: {message}"
    );
}

#[test]
fn integration_57_a_graphql_create_titled_extensions_warnings_empty() {
    let (_repo, server) = setup_ig_warn_demo();

    let result = server.graphql(
        r#"mutation { createDoogat(input: { title: "gql-explicit", type: "ig_warn_demo" }) { id title } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "createDoogat should succeed: {result}"
    );
    assert_eq!(
        result["extensions"]["warnings"],
        serde_json::json!([]),
        "expected empty extensions.warnings: {result}"
    );
    assert!(result.get("data").is_some(), "expected data key: {result}");
}

#[test]
fn integration_57_b_graphql_create_omitted_title_title_from_template_warning() {
    let (_repo, server) = setup_ig_warn_demo();

    let result = server
        .graphql(r#"mutation { createDoogat(input: { type: "ig_warn_demo" }) { id title } }"#);
    assert!(
        result.get("errors").is_none(),
        "createDoogat should succeed: {result}"
    );
    let warnings = result["extensions"]["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("extensions.warnings should be an array: {result}"));
    assert!(result.get("data").is_some(), "expected data key: {result}");
    assert_eq!(warnings.len(), 1, "expected exactly 1 warning: {result}");
    assert_eq!(
        warnings[0]["code"].as_str(),
        Some("TITLE_FROM_TEMPLATE"),
        "unexpected warning code: {result}"
    );
    let message = warnings[0]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("warning message should be a string: {result}"));
    assert!(
        message.contains("title_template"),
        "warning message should mention title_template: {message}"
    );
}

#[test]
fn integration_58_a_graphql_update_warnings_empty() {
    let (_repo, server) = setup_ig_upd_demo();

    let create = server.graphql(
        r#"mutation { createDoogat(input: { title: "upd-orig", type: "ig_upd_demo" }) { id } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "createDoogat should succeed: {create}"
    );
    let id = create["data"]["createDoogat"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("created doogat should have an id: {create}"));

    let update = server.graphql(&format!(
        r#"mutation {{ updateDoogat(input: {{ id: "{id}", title: "upd-changed" }}) {{ id title }} }}"#
    ));
    assert!(
        update.get("errors").is_none(),
        "updateDoogat should succeed: {update}"
    );
    assert_eq!(
        update["extensions"]["warnings"],
        serde_json::json!([]),
        "expected empty extensions.warnings: {update}"
    );
    assert_eq!(
        update["data"]["updateDoogat"]["title"].as_str(),
        Some("upd-changed"),
        "update should apply: {update}"
    );
}

#[test]
fn integration_58_b_graphql_update_missing_id_not_found() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(
        r#"mutation { updateDoogat(input: { id: "99990101000000", title: "nope" }) { id } }"#,
    );
    assert!(
        result.get("errors").is_some(),
        "update on missing id should error: {result}"
    );
    assert_eq!(
        result["errors"][0]["extensions"]["code"].as_str(),
        Some("NOT_FOUND"),
        "unexpected extensions.code: {result}"
    );
}

#[test]
fn integration_58_c_rest_update_warnings_empty() {
    let (_repo, server) = setup_ig_upd_demo();

    let create = server.graphql(
        r#"mutation { createDoogat(input: { title: "upd-orig", type: "ig_upd_demo" }) { id } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "createDoogat should succeed: {create}"
    );
    let id = create["data"]["createDoogat"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("created doogat should have an id: {create}"));

    let response = server.rest_put(
        &format!("/doogats/{id}"),
        serde_json::json!({ "title": "upd-rest" }),
    );
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().expect("invalid json");
    assert_eq!(
        body["warnings"],
        serde_json::json!([]),
        "expected empty warnings array: {body}"
    );
    assert_eq!(
        body["data"]["title"].as_str(),
        Some("upd-rest"),
        "update should apply: {body}"
    );
}

#[test]
fn integration_58_d_rest_update_missing_id_not_found() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let response = server.rest_put(
        "/doogats/99990101000000",
        serde_json::json!({ "title": "nope" }),
    );
    assert_eq!(response.status().as_u16(), 404);
    let body: serde_json::Value = response.json().expect("invalid json");
    assert_eq!(
        body["error"].as_str(),
        Some("NOT_FOUND"),
        "unexpected error field: {body}"
    );
}
