use crate::common::{ServerGuard, DdbTestRepo};
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
