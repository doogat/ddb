//! Port of tests/integration.sh:2188-2281 (PRD 00139 T20/T22 SINGLETON
//! GraphQL flow, ALTER SET/DROP SINGLETON, DEFAULT VALUES auto-seed).

use crate::common::{select_scalar, DdbTestRepo, ServerGuard};

/// Run a DDL/DML statement via `executeSql`, selecting `message`.
fn execute_sql(server: &ServerGuard, sql: &str) -> serde_json::Value {
    server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": sql }),
    )
}

#[test]
fn integration_51_a_singleton_graphql_flow() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = execute_sql(&server, "CREATE TABLE ig_app_config (theme TEXT) SINGLETON");
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE ... SINGLETON failed: {create}"
    );
    let create_message = create["data"]["executeSql"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        create_message.contains("ig_app_config"),
        "expected create message to mention ig_app_config, got: {create_message}"
    );

    let empty_query = server.graphql(r#"{ ig_app_config { id theme } }"#);
    assert!(
        empty_query.get("errors").is_none(),
        "query before any row exists failed: {empty_query}"
    );
    assert!(
        empty_query["data"]["ig_app_config"].is_null(),
        "singleton with no row should return null: {empty_query}"
    );

    let update_before_row = server.graphql(
        r#"mutation { update_ig_app_config(input: "{\"theme\":\"dark\"}") { id theme } }"#,
    );
    assert!(
        update_before_row.get("errors").is_some(),
        "update on an empty singleton should error: {update_before_row}"
    );
    assert_eq!(
        update_before_row["errors"][0]["extensions"]["code"].as_str(),
        Some("SINGLETON_NOT_FOUND"),
        "unexpected extensions.code: {update_before_row}"
    );

    let first_upsert = server.graphql(
        r#"mutation { upsert_ig_app_config(input: "{\"theme\":\"dark\"}") { id created } }"#,
    );
    assert!(
        first_upsert.get("errors").is_none(),
        "first upsert should succeed: {first_upsert}"
    );
    assert_eq!(
        first_upsert["data"]["upsert_ig_app_config"]["created"].as_bool(),
        Some(true),
        "first upsert should create the row: {first_upsert}"
    );
    let id = first_upsert["data"]["upsert_ig_app_config"]["id"]
        .as_str()
        .expect("first upsert should return an id")
        .to_string();

    let after_first_upsert = server.graphql(r#"{ ig_app_config { theme } }"#);
    assert!(
        after_first_upsert.get("errors").is_none(),
        "query after first upsert failed: {after_first_upsert}"
    );
    assert_eq!(
        after_first_upsert["data"]["ig_app_config"]["theme"].as_str(),
        Some("dark"),
        "unexpected theme after first upsert: {after_first_upsert}"
    );

    let second_upsert = server.graphql(
        r#"mutation { upsert_ig_app_config(input: "{\"theme\":\"light\"}") { id created } }"#,
    );
    assert!(
        second_upsert.get("errors").is_none(),
        "second upsert should succeed: {second_upsert}"
    );
    assert_eq!(
        second_upsert["data"]["upsert_ig_app_config"]["created"].as_bool(),
        Some(false),
        "second upsert should update, not create: {second_upsert}"
    );
    assert_eq!(
        second_upsert["data"]["upsert_ig_app_config"]["id"].as_str(),
        Some(id.as_str()),
        "second upsert should reuse the same row id: {second_upsert}"
    );

    let update = server.graphql(
        r#"mutation { update_ig_app_config(input: "{\"theme\":\"auto\"}") { id theme } }"#,
    );
    assert!(update.get("errors").is_none(), "update failed: {update}");
    assert_eq!(
        update["data"]["update_ig_app_config"]["id"].as_str(),
        Some(id.as_str()),
        "update should target the same row: {update}"
    );
    assert_eq!(
        update["data"]["update_ig_app_config"]["theme"].as_str(),
        Some("auto"),
        "unexpected theme after update: {update}"
    );

    let second_row = server.graphql(
        r#"mutation { createDoogat(input: {type: "ig_app_config", title: "x", fields: "{\"theme\":\"system\"}"}) { id } }"#,
    );
    assert!(
        second_row.get("errors").is_some(),
        "createDoogat on a filled SINGLETON typedef should error: {second_row}"
    );
    assert_eq!(
        second_row["errors"][0]["extensions"]["code"].as_str(),
        Some("SINGLETON_VIOLATION"),
        "unexpected extensions.code: {second_row}"
    );
}

#[test]
fn integration_52_b_graphql_alter_set_drop_singleton_reload() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = execute_sql(&server, "CREATE TABLE ig_alter_cfg (theme TEXT)");
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE failed: {create}"
    );

    let insert = execute_sql(
        &server,
        "INSERT INTO ig_alter_cfg (title, theme) VALUES ('cfg-a', 'dark')",
    );
    assert!(insert.get("errors").is_none(), "INSERT failed: {insert}");

    let set_singleton = execute_sql(&server, "ALTER TABLE ig_alter_cfg SET SINGLETON");
    assert!(
        set_singleton.get("errors").is_none(),
        "ALTER TABLE SET SINGLETON failed: {set_singleton}"
    );

    let schema_after_set = server.graphql(r#"{ __schema { queryType { fields { name } } } }"#);
    assert!(
        schema_after_set.get("errors").is_none(),
        "schema introspection after SET SINGLETON failed: {schema_after_set}"
    );
    let fields_after_set: Vec<&str> = schema_after_set["data"]["__schema"]["queryType"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(
        fields_after_set.contains(&"ig_alter_cfg"),
        "expected singular ig_alter_cfg query field after SET SINGLETON, got: {fields_after_set:?}"
    );

    let drop_singleton = execute_sql(&server, "ALTER TABLE ig_alter_cfg DROP SINGLETON");
    assert!(
        drop_singleton.get("errors").is_none(),
        "ALTER TABLE DROP SINGLETON failed: {drop_singleton}"
    );

    let schema_after_drop = server.graphql(r#"{ __schema { queryType { fields { name } } } }"#);
    assert!(
        schema_after_drop.get("errors").is_none(),
        "schema introspection after DROP SINGLETON failed: {schema_after_drop}"
    );
    let fields_after_drop: Vec<&str> = schema_after_drop["data"]["__schema"]["queryType"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(
        !fields_after_drop.contains(&"ig_alter_cfg"),
        "expected ig_alter_cfg query field removed after DROP SINGLETON, got: {fields_after_drop:?}"
    );
}

#[test]
fn integration_53_c_singleton_default_values_auto_seed() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = execute_sql(
        &server,
        "CREATE TABLE ig_seed_cfg (theme TEXT DEFAULT 'system', schema_version INTEGER DEFAULT 1) SINGLETON DEFAULT VALUES",
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE ... SINGLETON DEFAULT VALUES failed: {create}"
    );

    let count = select_scalar(&server, "SELECT COUNT(*) FROM ig_seed_cfg");
    assert_eq!(
        count, "1",
        "expected exactly one auto-seeded row from DEFAULT VALUES"
    );

    let theme = select_scalar(&server, "SELECT theme FROM ig_seed_cfg");
    assert_eq!(theme, "system", "expected DEFAULT-clause theme value");

    let schema_version = select_scalar(&server, "SELECT schema_version FROM ig_seed_cfg");
    assert_eq!(
        schema_version, "1",
        "expected DEFAULT-clause schema_version value"
    );
}
