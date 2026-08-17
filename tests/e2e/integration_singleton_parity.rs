//! Cross-protocol SINGLETON duplicate-insert parity (integration.sh §54.D / integration.ps1 §54.D).
//!
//! One seeded row in a SINGLETON table, then a duplicate insert attempted on
//! every remaining public transport. The CLI leg lives in
//! `alter_singleton::after_set_singleton_second_insert_rejects_with_constraint`
//! and the PgWire leg in
//! `pgwire_singleton::pgwire_singleton_duplicate_insert_keeps_connection_healthy`;
//! this test covers the GraphQL (`executeSql` *and* typed `createDoogat`), REST,
//! and NoSQL-HTTP legs plus the shared end state.

use crate::common::{DdbTestRepo, ServerGuard};
use predicates::prelude::*;
use serde_json::{json, Value};

/// Read `errors[0].extensions.{code,table}` off a GraphQL response, failing with
/// the whole response body when either is missing.
fn singleton_error_extensions(response: &Value, label: &str) -> (String, String) {
    let errors = response["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("{label} must return GraphQL errors, got: {response}"));
    assert!(
        !errors.is_empty(),
        "{label} must return at least one GraphQL error, got: {response}"
    );
    let extensions = &errors[0]["extensions"];
    let code = extensions["code"]
        .as_str()
        .unwrap_or_else(|| panic!("{label} error missing extensions.code, got: {response}"))
        .to_string();
    let table = extensions["table"]
        .as_str()
        .unwrap_or_else(|| panic!("{label} error missing extensions.table, got: {response}"))
        .to_string();
    (code, table)
}

#[test]
fn integration_54_d_cross_protocol_singleton_duplicate_parity() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Seed: a SINGLETON table holding exactly one row.
    repo.ddb()
        .args(["query", "CREATE TABLE ig_parity_cfg (theme TEXT) SINGLETON"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table ig_parity_cfg created"));

    let seed = repo
        .ddb()
        .args([
            "query",
            "INSERT INTO ig_parity_cfg (title, theme) VALUES ('seed', 'p1')",
        ])
        .assert()
        .success();
    let existing_id = String::from_utf8_lossy(&seed.get_output().stdout)
        .trim()
        .to_string();
    assert!(
        existing_id.len() == 14 && existing_id.chars().all(|c| c.is_ascii_digit()),
        "seed INSERT must print a 14-digit doogat id, got: {existing_id:?}"
    );

    // Control: an ordinary table declared WITHOUT SINGLETON takes as many rows as
    // it is given. Without this, every rejection below would still pass on a
    // build that made *every* table single-row, since nothing else here proves
    // the constraint is tied to the declared SINGLETON condition. Both write
    // lanes the rejections travel are covered: SQL INSERT (the `executeSql` leg)
    // and typed create (the `createDoogat`/REST leg).
    repo.ddb()
        .args(["query", "CREATE TABLE ig_parity_plain (theme TEXT)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("table ig_parity_plain created"));
    repo.ddb()
        .args([
            "query",
            "INSERT INTO ig_parity_plain (title, theme) VALUES ('first', 'q1')",
        ])
        .assert()
        .success();
    repo.ddb()
        .args([
            "query",
            "INSERT INTO ig_parity_plain (title, theme) VALUES ('second', 'q2')",
        ])
        .assert()
        .success();
    let plain_typed = server.graphql(
        r#"mutation { createDoogat(input: { type: "ig_parity_plain", title: "third", fields: "{\"theme\":\"q3\"}" }) { id } }"#,
    );
    assert!(
        plain_typed.get("errors").is_none(),
        "typed create into a table declared without SINGLETON must succeed, got: {plain_typed}"
    );
    let plain_count = repo
        .ddb()
        .args(["query", "SELECT COUNT(*) FROM ig_parity_plain"])
        .assert()
        .success();
    let plain_stdout = String::from_utf8_lossy(&plain_count.get_output().stdout).into_owned();
    assert!(
        plain_stdout.lines().any(|line| line.trim() == "3"),
        "a table declared without SINGLETON must accept every row written to it, got: {plain_stdout}"
    );

    // GraphQL executeSql: duplicate INSERT rejected with a structured extension.
    let gql_sql = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO ig_parity_cfg (title, theme) VALUES ('x', 'p2')") { message affected rows } }"#,
    );
    let (sql_code, sql_table) = singleton_error_extensions(&gql_sql, "GraphQL executeSql");
    assert_eq!(
        sql_code, "SINGLETON_VIOLATION",
        "GraphQL executeSql duplicate INSERT must carry extensions.code=SINGLETON_VIOLATION, got: {gql_sql}"
    );
    assert_eq!(
        sql_table, "ig_parity_cfg",
        "GraphQL executeSql duplicate INSERT must name the table in extensions.table, got: {gql_sql}"
    );

    // GraphQL typed createDoogat: the same rejection, through the other mutation path.
    let gql_typed = server.graphql(
        r#"mutation { createDoogat(input: { type: "ig_parity_cfg", title: "x", fields: "{\"theme\":\"p3\"}" }) { id } }"#,
    );
    let (typed_code, typed_table) = singleton_error_extensions(&gql_typed, "GraphQL createDoogat");
    assert_eq!(
        typed_code, "SINGLETON_VIOLATION",
        "GraphQL createDoogat duplicate INSERT must carry extensions.code=SINGLETON_VIOLATION, got: {gql_typed}"
    );
    assert_eq!(
        typed_table, "ig_parity_cfg",
        "GraphQL createDoogat duplicate INSERT must name the table in extensions.table, got: {gql_typed}"
    );

    // The two GraphQL write paths must not diverge, even if each looks right alone.
    assert_eq!(
        sql_code, typed_code,
        "executeSql and createDoogat must return the identical singleton error code"
    );
    assert_eq!(
        sql_table, typed_table,
        "executeSql and createDoogat must return the identical singleton error table"
    );

    // REST: 409 with the shared code plus table and existing id in the envelope.
    let rest = server.rest_post("/doogats", json!({ "title": "x", "type": "ig_parity_cfg" }));
    assert_eq!(
        rest.status(),
        409,
        "REST duplicate create on a singleton must return 409 Conflict"
    );
    let rest_body: Value = rest.json().expect("invalid json");
    assert_eq!(
        rest_body["error"], "SINGLETON_VIOLATION",
        "REST error envelope must carry the SINGLETON_VIOLATION code, got: {rest_body}"
    );
    let rest_message = rest_body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("REST error envelope missing message, got: {rest_body}"));
    assert!(
        rest_message.contains("ig_parity_cfg"),
        "REST error message must name the singleton table, got: {rest_message}"
    );
    assert!(
        rest_message.contains(&existing_id),
        "REST error message must name the surviving row id {existing_id} (existing_id parity), got: {rest_message}"
    );

    // NoSQL HTTP is read-only today: it exposes only GET scan/get/backlinks routes,
    // so a typed write POST has no route to hit. Deferred gap, recorded here.
    let nosql = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{}/nosql", server.port))
        .header("Authorization", format!("Bearer {}", server.token))
        .json(&json!({ "title": "x", "type": "ig_parity_cfg" }))
        .send()
        .expect("NoSQL POST request failed");
    let nosql_status = nosql.status().as_u16();
    assert!(
        nosql_status == 404 || nosql_status == 405,
        "NoSQL HTTP must expose no typed write route (expected 404 or 405), got: {nosql_status}"
    );

    // End state: every duplicate attempt was rejected, so the seed row stands alone.
    let count = repo
        .ddb()
        .args(["query", "SELECT COUNT(*) FROM ig_parity_cfg"])
        .assert()
        .success();
    let count_stdout = String::from_utf8_lossy(&count.get_output().stdout).into_owned();
    assert!(
        count_stdout.lines().any(|line| line.trim() == "1"),
        "singleton must still hold exactly one row after the rejected duplicates, got: {count_stdout}"
    );
    repo.ddb()
        .args(["query", "SELECT theme FROM ig_parity_cfg"])
        .assert()
        .success()
        .stdout(predicate::str::contains("p1"));
}
