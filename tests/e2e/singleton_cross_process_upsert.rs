//! Cross-process SINGLETON write safety for upsert and PgWire-INSERT paths (PRD 00140).
//!
//! Two concurrent `upsert_<type>` GraphQL mutations and two concurrent PgWire INSERTs
//! race against the same SINGLETON typedef served by a running `ddb serve`.
//! The upsert race must produce exactly one `created:true` and one `created:false`,
//! both returning the same id, with no errors. The PgWire race must produce exactly
//! one Ok and one structured SINGLETON error, with a single row materialised.

use crate::common::{DdbTestRepo, ServerGuard};
use tokio_postgres::{Client, SimpleQueryMessage};

#[test]
fn two_concurrent_graphql_upserts_on_singleton_converge_on_one_row() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE app_config (theme TEXT) SINGLETON"])
        .assert()
        .success();
    let server = ServerGuard::start(&repo);

    let (r1, r2) = std::thread::scope(|s| {
        let t1 = s.spawn(|| {
            server.graphql(
                r#"mutation { upsert_app_config(input: "{\"theme\":\"dark\"}") { id created } }"#,
            )
        });
        let t2 = s.spawn(|| {
            server.graphql(
                r#"mutation { upsert_app_config(input: "{\"theme\":\"dark\"}") { id created } }"#,
            )
        });
        (t1.join().unwrap(), t2.join().unwrap())
    });

    assert!(r1.get("errors").is_none(), "upsert 1 had errors: {r1}");
    assert!(r2.get("errors").is_none(), "upsert 2 had errors: {r2}");

    let created1 = r1["data"]["upsert_app_config"]["created"]
        .as_bool()
        .expect("missing created in r1");
    let created2 = r2["data"]["upsert_app_config"]["created"]
        .as_bool()
        .expect("missing created in r2");
    let id1 = r1["data"]["upsert_app_config"]["id"]
        .as_str()
        .expect("missing id in r1");
    let id2 = r2["data"]["upsert_app_config"]["id"]
        .as_str()
        .expect("missing id in r2");

    assert_ne!(
        created1, created2,
        "expected exactly one created:true and one created:false; got {created1} and {created2}"
    );
    assert_eq!(id1, id2, "both upserts must return the same id");

    // Verify exactly one row exists with the expected theme.
    let query_resp = server.graphql(r#"{ app_config { id theme } }"#);
    assert!(
        query_resp.get("errors").is_none(),
        "app_config query failed: {query_resp}"
    );
    let row = &query_resp["data"]["app_config"];
    assert!(!row.is_null(), "expected exactly one app_config row, got null");
    assert_eq!(
        row["theme"].as_str().unwrap_or(""),
        "dark",
        "expected theme=dark, got: {row}"
    );
}

#[test]
fn two_concurrent_pgwire_inserts_on_singleton_one_wins_one_gets_singleton_error() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE pg_cfg (theme TEXT) SINGLETON"])
        .assert()
        .success();
    let server = ServerGuard::start(&repo);

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (client1, conn1) = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(server.pg_port)
            .user("ddb")
            .password(&server.token)
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            conn1.await.ok();
        });

        let (client2, conn2) = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(server.pg_port)
            .user("ddb")
            .password(&server.token)
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            conn2.await.ok();
        });

        let (res1, res2) = tokio::join!(
            client1.simple_query("INSERT INTO pg_cfg (title, theme) VALUES ('a', 'dark')"),
            client2.simple_query("INSERT INTO pg_cfg (title, theme) VALUES ('b', 'dark')"),
        );

        let res1_ok = res1.is_ok();
        let res2_ok = res2.is_ok();
        let ok_count = [res1_ok, res2_ok].iter().filter(|&&ok| ok).count();
        assert_eq!(ok_count, 1, "expected exactly one successful insert");
        assert_eq!(2 - ok_count, 1, "expected exactly one failed insert");

        let err = if res1.is_err() {
            res1.unwrap_err()
        } else {
            res2.unwrap_err()
        };
        let err_msg = err
            .as_db_error()
            .map(|e| e.message().to_string())
            .unwrap_or_else(|| err.to_string());
        assert!(
            err_msg.contains("SINGLETON constraint") || err_msg.contains("SINGLETON_VIOLATION"),
            "expected structured singleton error, got: {err_msg}"
        );
        assert!(
            !err_msg.contains("UNIQUE constraint failed"),
            "raw SQLite UNIQUE error must not leak; got: {err_msg}"
        );

        // Verify exactly one row using a fresh connection.
        let (count_client, count_conn) = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(server.pg_port)
            .user("ddb")
            .password(&server.token)
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            count_conn.await.ok();
        });
        let count = pg_count(&count_client, "SELECT COUNT(*) FROM pg_cfg").await;
        assert_eq!(count, 1, "expected exactly 1 row in pg_cfg");
    });
}

async fn pg_count(client: &Client, sql: &str) -> u64 {
    client
        .simple_query(sql)
        .await
        .expect("count query should succeed")
        .into_iter()
        .find_map(|msg| match msg {
            SimpleQueryMessage::Row(row) => row.get(0).and_then(|v| v.parse::<u64>().ok()),
            _ => None,
        })
        .expect("missing count row")
}
