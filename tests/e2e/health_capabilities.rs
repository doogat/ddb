use crate::common::{DdbTestRepo, ServerGuard};
use std::time::Duration;

/// Mirror of `ddb_server::capabilities::FLAGS`; the e2e crate cannot import
/// ddb-server, so the published flag set is pinned here.
const EXPECTED_FLAGS: [&str; 6] = [
    "typed_orderby_base_fields",
    "relation_where_filters",
    "inline_zone_ddl",
    "schema_apply",
    "offline_sdl",
    "capabilities_handshake",
];

fn get_health(server: &ServerGuard, path: &str) -> serde_json::Value {
    reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}{path}", server.port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("health request failed")
        .json()
        .expect("health body must be JSON")
}

fn string_list(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("expected a flat JSON array of strings; got: {v}"))
        .iter()
        .map(|s| {
            s.as_str()
                .unwrap_or_else(|| panic!("each capability must be a string; got: {s}"))
                .to_string()
        })
        .collect()
}

#[test]
fn health_lists_capability_flags_in_order_and_keeps_version() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let body = get_health(&server, "/health");
    assert!(
        !body["capabilities"].is_object(),
        "capabilities must be a list of names, not an object of booleans; got: {body}"
    );
    assert_eq!(string_list(&body["capabilities"]), EXPECTED_FLAGS);
    assert_eq!(body["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(body["status"].as_str(), Some("ok"));
}

#[test]
fn health_ready_route_also_lists_capabilities() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let body = get_health(&server, "/health/ready");
    assert_eq!(string_list(&body["capabilities"]), EXPECTED_FLAGS);
}

#[test]
fn health_and_graphql_report_same_version_and_flags() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let health = get_health(&server, "/health");
    let gql = server.graphql("{ capabilities { version flags } }");
    assert!(gql["errors"].is_null(), "unexpected GraphQL errors: {gql}");
    let caps = &gql["data"]["capabilities"];

    assert_eq!(caps["version"], health["version"], "gql={caps} health={health}");
    assert_eq!(
        string_list(&caps["flags"]),
        string_list(&health["capabilities"])
    );
    let names = string_list(&health["capabilities"]);
    assert!(!names.iter().any(|n| n == "schema" || n == "offline"));
}
