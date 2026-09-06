//! ALTER TABLE RENAME across protocols (PRD 00132).
//! CLI-transport coverage already exists in
//! `tests/e2e/alter_table_rename.rs`; these tests cover the GraphQL and
//! PgWire transports.

use crate::common::{DdbTestRepo, ServerGuard};
use tokio_postgres::SimpleQueryMessage;

/// Run a DDL/DML statement via `executeSql`, selecting `message`.
fn ddl(server: &ServerGuard, sql: &str) -> serde_json::Value {
    server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": sql }),
    )
}

/// Run a SELECT via `executeSql`, selecting `rows`.
fn select(server: &ServerGuard, sql: &str) -> serde_json::Value {
    server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { rows } }"#,
        serde_json::json!({ "sql": sql }),
    )
}

#[test]
fn integration_50a_alter_rename_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = ddl(&server, "CREATE TABLE rngql_src (title VARCHAR(64))");
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE failed: {create}"
    );

    let rename = ddl(&server, "ALTER TABLE rngql_src RENAME TO rngql_dst");
    assert!(
        rename.get("errors").is_none(),
        "ALTER TABLE RENAME TO failed: {rename}"
    );

    let new_name = select(&server, "SELECT count(*) FROM rngql_dst");
    assert!(
        new_name.get("data").is_some(),
        "new table must return data: {new_name}"
    );
    assert!(
        new_name.get("errors").is_none(),
        "SELECT on new table name should succeed: {new_name}"
    );

    let old_name = select(&server, "SELECT count(*) FROM rngql_src");
    assert!(
        old_name.get("errors").is_some(),
        "SELECT on old table name should error: {old_name}"
    );
}

#[test]
fn integration_50b_mysql_rename_alias_rejected_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = ddl(&server, "CREATE TABLE rngql_alias_src (title VARCHAR(64))");
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE failed: {create}"
    );

    let rename = ddl(&server, "RENAME TABLE rngql_alias_src TO rngql_alias_dst");
    assert!(
        rename.get("errors").is_some(),
        "MySQL RENAME TABLE alias should be rejected: {rename}"
    );
    let message = rename["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("RENAME TABLE not supported"),
        "expected 'RENAME TABLE not supported' in error message, got: {message}"
    );
}

#[test]
fn integration_50c_alter_rename_pgwire() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (client, connection) = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(server.pg_port)
            .user("ddb")
            .password(&server.token)
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            connection.await.ok();
        });

        client
            .simple_query("CREATE TABLE rnpg_src (title VARCHAR(64))")
            .await
            .unwrap();

        client
            .simple_query("ALTER TABLE rnpg_src RENAME TO rnpg_dst")
            .await
            .unwrap();

        let messages = client
            .simple_query("SELECT count(*) FROM rnpg_dst")
            .await
            .unwrap();
        let row = messages
            .iter()
            .find_map(|m| match m {
                SimpleQueryMessage::Row(row) => Some(row),
                _ => None,
            })
            .expect("missing row for SELECT count(*) FROM rnpg_dst");
        assert_eq!(row.get(0), Some("0"));
    });
}
