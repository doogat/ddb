//! SINGLETON insert races, each asserting the exact final row count.
//! The CLI-only leg is
//! `singleton_cross_process_create::two_concurrent_ddb_create_on_singleton_converge_on_one_row`.

use crate::common::{assert_doogat_id, DdbTestRepo, ServerGuard};
use serde_json::Value;
use std::process::Command;
use std::sync::Barrier;
use std::time::Duration;
use tokio_postgres::SimpleQueryMessage;

fn race_graphql(server: &ServerGuard, mutations: &[String; 2]) -> [Value; 2] {
    let ready = Barrier::new(3);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            ready.wait();
            server.graphql(&mutations[0])
        });
        let second = scope.spawn(|| {
            ready.wait();
            server.graphql(&mutations[1])
        });
        ready.wait();
        [
            first.join().expect("first GraphQL writer panicked"),
            second.join().expect("second GraphQL writer panicked"),
        ]
    })
}

fn graphql_winner(responses: &[Value; 2], table: &str) -> usize {
    for response in responses {
        assert!(
            !response
                .to_string()
                .to_ascii_lowercase()
                .contains("unique constraint failed"),
            "raw SQLite UNIQUE error leaked: {response}"
        );
    }
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.get("errors").is_none())
            .count(),
        1,
        "expected exactly one GraphQL success: {responses:?}"
    );
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.get("errors").is_some())
            .count(),
        1,
        "expected exactly one GraphQL error response: {responses:?}"
    );
    let winner = usize::from(responses[0].get("errors").is_some());
    let loser = &responses[1 - winner];
    let errors = loser["errors"].as_array().expect("errors must be an array");
    assert!(!errors.is_empty(), "loser has no error: {loser}");
    assert_eq!(errors[0]["extensions"]["code"], "SINGLETON_VIOLATION");
    assert_eq!(errors[0]["extensions"]["table"], table);
    winner
}

/// Bind §55.E's materialized count to the sole committed row and its payload.
fn assert_survivor(repo: &DdbTestRepo, table: &str, theme: &str) -> String {
    let count = repo
        .ddb()
        .args(["query", &format!("SELECT COUNT(*) FROM {table}")])
        .assert()
        .success();
    assert_eq!(
        String::from_utf8_lossy(&count.get_output().stdout).trim(),
        "1",
        "§55.E: {table} must converge on exactly one row"
    );
    let row_id = repo
        .ddb()
        .args(["query", &format!("SELECT id FROM {table}")])
        .assert()
        .success();
    let id = String::from_utf8_lossy(&row_id.get_output().stdout)
        .trim()
        .to_owned();
    assert_doogat_id(&id);
    let row_theme = repo
        .ddb()
        .args(["query", &format!("SELECT theme FROM {table}")])
        .assert()
        .success();
    assert_eq!(
        String::from_utf8_lossy(&row_theme.get_output().stdout).trim(),
        theme,
        "surviving row must match the successful writer"
    );

    let tree = Command::new("git")
        .current_dir(repo.path())
        .args(["ls-tree", "--name-only", "HEAD:ddb"])
        .output()
        .expect("git ls-tree failed to run");
    assert!(tree.status.success(), "git ls-tree failed: {tree:?}");
    let tree_stdout = String::from_utf8_lossy(&tree.stdout);
    let committed_ids: Vec<&str> = tree_stdout
        .lines()
        .filter_map(|path| path.strip_suffix(".md"))
        .collect();
    assert_eq!(
        committed_ids,
        [id.as_str()],
        "HEAD must contain exactly the surviving row"
    );
    let committed = Command::new("git")
        .current_dir(repo.path())
        .args(["show", &format!("HEAD:ddb/{id}.md")])
        .output()
        .expect("git show failed to run");
    assert!(committed.status.success(), "git show failed: {committed:?}");
    assert!(
        String::from_utf8_lossy(&committed.stdout).contains(theme),
        "winner's theme {theme:?} must be durable in HEAD: {committed:?}"
    );
    id
}

#[test]
fn integration_55_b_graphql_execute_sql_singleton_insert_race() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE ig_race_sql (theme TEXT) SINGLETON"])
        .assert()
        .success();
    let server = ServerGuard::start(&repo);
    let themes = ["sql-dark", "sql-light"];
    let mutations = themes.map(|theme| {
        format!(
            r#"mutation {{ executeSql(sql: "INSERT INTO ig_race_sql (title, theme) VALUES ('x', '{theme}')") {{ message }} }}"#
        )
    });
    let responses = race_graphql(&server, &mutations);
    let winner = graphql_winner(&responses, "ig_race_sql");
    let result = &responses[winner]["data"]["executeSql"];
    let winner_id = result["message"]
        .as_str()
        .expect("INSERT must return an id");
    let survivor = assert_survivor(&repo, "ig_race_sql", themes[winner]);
    assert_eq!(survivor, winner_id.trim());

    // §55.B also requires the generated singleton GraphQL field to stay readable.
    let query = server.graphql("{ ig_race_sql { id theme } }");
    assert!(
        query.get("errors").is_none(),
        "singleton query failed: {query}"
    );
    assert_eq!(query["data"]["ig_race_sql"]["id"], survivor);
    assert_eq!(query["data"]["ig_race_sql"]["theme"], themes[winner]);
}

#[test]
fn integration_55_c_graphql_create_doogat_singleton_insert_race() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE ig_race_typed (theme TEXT) SINGLETON"])
        .assert()
        .success();
    let server = ServerGuard::start(&repo);
    let themes = ["typed-dark", "typed-light"];
    let mutations = themes.map(|theme| {
        format!(
            r#"mutation {{ createDoogat(input: {{ type: "ig_race_typed", title: "x", fields: "{{\"theme\":\"{theme}\"}}" }}) {{ id }} }}"#
        )
    });
    let responses = race_graphql(&server, &mutations);
    let winner = graphql_winner(&responses, "ig_race_typed");
    let winner_id = responses[winner]["data"]["createDoogat"]["id"]
        .as_str()
        .expect("typed winner must return the created id");
    assert_eq!(
        assert_survivor(&repo, "ig_race_typed", themes[winner]),
        winner_id
    );
}

#[test]
fn integration_55_d_cli_vs_pgwire_singleton_insert_race() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", "CREATE TABLE ig_race_pg (theme TEXT) SINGLETON"])
        .assert()
        .success();
    let server = ServerGuard::start(&repo);
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime failed");
    let (client, connection) = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio_postgres::Config::new()
                .host("127.0.0.1")
                .port(server.pg_port)
                .user("ddb")
                .password(&server.token)
                .dbname("ddb")
                .connect(tokio_postgres::NoTls),
        )
        .await
        .expect("PgWire connect timed out")
        .expect("PgWire connect failed")
    });
    let connection_task = runtime.spawn(connection);

    let ready = Barrier::new(2);
    let (cli, pg) = std::thread::scope(|scope| {
        let cli_writer = scope.spawn(|| {
            ready.wait();
            repo.ddb()
                .args([
                    "create",
                    "--type",
                    "ig_race_pg",
                    "--title",
                    "x",
                    "--set",
                    "theme=cli-dark",
                ])
                .timeout(Duration::from_secs(30))
                .output()
                .expect("CLI writer failed to run")
        });
        ready.wait();
        let pg = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(30),
                client
                    .simple_query("INSERT INTO ig_race_pg (title, theme) VALUES ('x', 'pg-light')"),
            )
            .await
            .expect("PgWire INSERT timed out")
        });
        (cli_writer.join().expect("CLI writer panicked"), pg)
    });

    let successes = [cli.status.success(), pg.is_ok()];
    assert_eq!(
        successes.iter().filter(|&&ok| ok).count(),
        1,
        "expected one winner: CLI={cli:?}, PgWire={pg:?}"
    );
    assert_eq!(
        successes.iter().filter(|&&ok| !ok).count(),
        1,
        "expected one loser: CLI={cli:?}, PgWire={pg:?}"
    );
    let loser_message = match &pg {
        Ok(messages) => {
            assert!(
                messages
                    .iter()
                    .any(|message| matches!(message, SimpleQueryMessage::CommandComplete(1))),
                "PgWire winner must report one inserted row: {messages:?}"
            );
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&cli.stdout),
                String::from_utf8_lossy(&cli.stderr)
            )
        }
        Err(error) => error
            .as_db_error()
            .expect("PgWire loser must return a database error, not lose the connection")
            .message()
            .to_owned(),
    };
    let normalized = loser_message.to_ascii_lowercase();
    assert!(
        !normalized.contains("unique constraint failed"),
        "raw SQLite UNIQUE error leaked: {loser_message}"
    );
    // §55.D deliberately allows recognized transient contention. Do not turn
    // this into the stronger, actor-serialized §55.B/C error contract.
    assert!(
        [
            "singleton constraint",
            "locked",
            "busy",
            "index.lock",
            "cannot lock",
            "unable to create",
            "file exists",
            "another git process",
        ]
        .iter()
        .any(|fragment| normalized.contains(fragment)),
        "unrecognized CLI/PgWire loser error: {loser_message}"
    );

    // Use the same PgWire connection after either outcome, including a rejected
    // INSERT. Both protocol convergence and connection usability must hold.
    let count = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(30),
            client.simple_query("SELECT COUNT(*) FROM ig_race_pg"),
        )
        .await
        .expect("PgWire count timed out")
        .expect("PgWire connection must remain usable after the race")
    });
    let rows: Vec<_> = count
        .iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(row),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 1, "COUNT must return exactly one result row");
    assert_eq!(
        rows[0].get(0),
        Some("1"),
        "§55.E: PgWire must see one survivor"
    );

    let theme = if cli.status.success() {
        "cli-dark"
    } else {
        "pg-light"
    };
    let survivor = assert_survivor(&repo, "ig_race_pg", theme);
    if cli.status.success() {
        assert_eq!(String::from_utf8_lossy(&cli.stdout).trim(), survivor);
    }
    drop(client);
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), connection_task)
            .await
            .expect("PgWire connection shutdown timed out")
            .expect("PgWire driver panicked")
            .expect("PgWire driver failed");
    });
}
