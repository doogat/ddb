use crate::common::{DdbTestRepo, ServerGuard};
use std::collections::HashSet;
use tokio_postgres::SimpleQueryMessage;

#[test]
fn pgwire_connect_auth_ok() {
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

        let messages = client.simple_query("SELECT 1").await.unwrap();
        let row = messages
            .iter()
            .find_map(|m| match m {
                SimpleQueryMessage::Row(row) => Some(row),
                _ => None,
            })
            .expect("missing row");
        assert_eq!(row.get(0), Some("1"));
    });
}

#[test]
fn pgwire_auth_rejected() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let connect = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(server.pg_port)
            .user("ddb")
            .password("wrong-token")
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await;
        assert!(connect.is_err(), "expected auth to fail");
    });
}

#[test]
fn pgwire_select_with_columns() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title } }"#,
        serde_json::json!({
            "input": {
                "title": "PGWire Note",
                "content": "row content"
            }
        }),
    );
    assert!(create.get("errors").is_none(), "create failed: {create}");
    let created = &create["data"]["createDoogat"];
    let id = created["id"].as_str().unwrap().to_string();
    let title = created["title"].as_str().unwrap().to_string();

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

        let messages = client
            .simple_query("SELECT id, title FROM doogats")
            .await
            .unwrap();
        let row = messages
            .iter()
            .find_map(|m| match m {
                SimpleQueryMessage::Row(row) if row.get(0) == Some(id.as_str()) => Some(row),
                _ => None,
            })
            .expect("missing created row");
        assert_eq!(row.columns()[0].name(), "id");
        assert_eq!(row.columns()[1].name(), "title");
        assert_eq!(row.get(0), Some(id.as_str()));
        assert_eq!(row.get(1), Some(title.as_str()));
    });
}

#[test]
fn pgwire_ddl_create_table() {
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
            .simple_query("CREATE TABLE book (title TEXT NOT NULL)")
            .await
            .unwrap();
    });

    let result = server.graphql(r#"{ books { items { id title } totalCount } }"#);
    assert!(
        result.get("errors").is_none(),
        "books query failed: {result}"
    );
    assert!(result["data"]["books"]["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(result["data"]["books"]["totalCount"].as_i64().unwrap(), 0);
}

#[test]
fn pgwire_insert_update_delete() {
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
            .simple_query("CREATE TABLE book (title TEXT NOT NULL)")
            .await
            .unwrap();

        let inserted = client
            .simple_query("INSERT INTO book (title) VALUES ('Dune')")
            .await
            .unwrap()
            .into_iter()
            .find_map(|m| match m {
                SimpleQueryMessage::CommandComplete(n) => Some(n),
                _ => None,
            })
            .expect("missing insert completion");
        assert!(inserted >= 1);

        let count_after_insert = client
            .simple_query("SELECT COUNT(*) FROM book")
            .await
            .unwrap()
            .into_iter()
            .find_map(|m| match m {
                SimpleQueryMessage::Row(row) => row.get(0).and_then(|v| v.parse::<u64>().ok()),
                _ => None,
            })
            .expect("missing row count");
        assert_eq!(count_after_insert, 1);

        let inserted_id = client
            .simple_query("SELECT id FROM book")
            .await
            .unwrap()
            .into_iter()
            .find_map(|m| match m {
                SimpleQueryMessage::Row(row) => row.get(0).map(|v| v.to_string()),
                _ => None,
            })
            .expect("missing inserted id");

        let updated = client
            .simple_query(&format!(
                "UPDATE book SET title = 'Dune Messiah' WHERE id = '{}'",
                inserted_id
            ))
            .await
            .unwrap()
            .into_iter()
            .find_map(|m| match m {
                SimpleQueryMessage::CommandComplete(n) => Some(n),
                _ => None,
            })
            .expect("missing update completion");
        assert_eq!(updated, 1);

        let deleted = client
            .simple_query(&format!("DELETE FROM book WHERE id = '{}'", inserted_id))
            .await
            .unwrap()
            .into_iter()
            .find_map(|m| match m {
                SimpleQueryMessage::CommandComplete(n) => Some(n),
                _ => None,
            })
            .expect("missing delete completion");
        assert_eq!(deleted, 1);
    });
}

#[test]
fn pgwire_pg_catalog_hides_internal_tables() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a user type table
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
            .simple_query("CREATE TABLE project (status TEXT)")
            .await
            .unwrap();

        // psql \dt sends a pg_catalog query - simulate it
        let messages = client
            .simple_query(
                "SELECT n.nspname as \"Schema\", c.relname as \"Name\", \
                 CASE c.relkind WHEN 'r' THEN 'table' END as \"Type\", \
                 pg_catalog.pg_get_userbyid(c.relowner) as \"Owner\" \
                 FROM pg_catalog.pg_class c \
                 LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE c.relkind IN ('r','p','') \
                 ORDER BY 1,2",
            )
            .await
            .unwrap();

        let table_names: HashSet<String> = messages
            .iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => row.get(1).map(|s| s.to_string()),
                _ => None,
            })
            .collect();

        // User table should be visible
        assert!(
            table_names.contains("project"),
            "user table 'project' should be listed, got: {table_names:?}"
        );

        // Internal tables should be hidden
        assert!(
            !table_names.contains("doogats"),
            "internal table 'doogats' should be hidden"
        );
        assert!(
            !table_names.contains("_ddb_tags"),
            "internal table '_ddb_tags' should be hidden"
        );
        assert!(
            !table_names.contains("_ddb_fts"),
            "internal table '_ddb_fts' should be hidden"
        );
        assert!(
            table_names.iter().all(|name| !name.starts_with("_ddb_")),
            "internal table leaked: {table_names:?}"
        );

        // Direct access to internal tables should still work
        let direct = client
            .simple_query("SELECT COUNT(*) FROM _ddb_tags")
            .await
            .expect("direct query on internal table should still work");
        let count = direct.iter().find_map(|message| match message {
            SimpleQueryMessage::Row(row) => row.get(0),
            _ => None,
        });
        assert!(
            count.unwrap().parse::<u64>().is_ok(),
            "invalid count: {direct:?}"
        );
    });
}
