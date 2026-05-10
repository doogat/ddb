use crate::common::{DdbTestRepo, ServerGuard};
use tokio_postgres::{Client, SimpleQueryMessage};

#[test]
fn pgwire_singleton_duplicate_insert_keeps_connection_healthy() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create_singleton = server.graphql(
        r#"mutation { executeSql(sql:"CREATE TABLE app_cfg (theme TEXT) SINGLETON") { message } }"#,
    );
    assert!(
        create_singleton.get("errors").is_none(),
        "singleton create failed: {create_singleton}"
    );

    let seed_singleton = server.graphql(
        r#"mutation { executeSql(sql:"INSERT INTO app_cfg (title, theme) VALUES ('only', 'dark')") { message } }"#,
    );
    assert!(
        seed_singleton.get("errors").is_none(),
        "singleton seed failed: {seed_singleton}"
    );

    let create_regular = server.graphql(
        r#"mutation { executeSql(sql:"CREATE TABLE note_cfg (theme TEXT)") { message } }"#,
    );
    assert!(
        create_regular.get("errors").is_none(),
        "regular table create failed: {create_regular}"
    );

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

        let duplicate = client
            .simple_query("INSERT INTO app_cfg (title, theme) VALUES ('dup', 'light')")
            .await
            .expect_err("duplicate singleton insert should fail");
        let duplicate_message = duplicate
            .as_db_error()
            .map(|db_error| db_error.message().to_string())
            .unwrap_or_else(|| duplicate.to_string());
        assert!(
            duplicate_message.contains("SINGLETON constraint")
                || duplicate_message.contains("SINGLETON_VIOLATION"),
            "expected singleton constraint error, got: {duplicate_message}"
        );

        let inserted = command_complete_rows(
            client
                .simple_query("INSERT INTO note_cfg (title, theme) VALUES ('ok', 'blue')")
                .await
                .expect("regular insert after singleton failure should succeed"),
        );
        assert!(inserted >= 1, "expected regular insert to affect rows");

        let regular_count = query_count(&client, "SELECT COUNT(*) FROM note_cfg").await;
        assert_eq!(regular_count, 1);

        let singleton_count = query_count(&client, "SELECT COUNT(*) FROM app_cfg").await;
        assert_eq!(singleton_count, 1);
    });
}

fn command_complete_rows(messages: Vec<SimpleQueryMessage>) -> u64 {
    messages
        .into_iter()
        .find_map(|message| match message {
            SimpleQueryMessage::CommandComplete(rows) => Some(rows),
            _ => None,
        })
        .expect("missing command-complete message")
}

async fn query_count(client: &Client, sql: &str) -> u64 {
    client
        .simple_query(sql)
        .await
        .expect("count query should succeed")
        .into_iter()
        .find_map(|message| match message {
            SimpleQueryMessage::Row(row) => row.get(0).and_then(|value| value.parse::<u64>().ok()),
            _ => None,
        })
        .expect("missing count row")
}
