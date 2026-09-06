//! Hazard H6: the server's SQL transaction buffer is process-global, not
//! per-client.
//!
//! Evidence (static read, never executed): the actor owns ONE `DoogatService`
//! for every client (`ddb-server/src/actor/mod.rs:664-677`), and that service
//! carries the transaction buffer. `DoogatService::execute_sql` resumes and
//! suspends `self.txn` around every call and skips `ensure_fresh` while it is
//! set (`ddb-core/src/service/sql.rs:11-24`). PgWire routes every non-SELECT
//! statement — `BEGIN` included — to `actor.execute_sql`
//! (`ddb-server/src/pgwire.rs:102-114`), and GraphQL `executeSql` reaches the
//! same actor (`ddb-server/src/schema/mutations/operations.rs:262`). Once a
//! buffer exists, `buffer_or_collect_write`
//! (`ddb-core/src/sql_engine/dml.rs:377-397`) parks writes in it instead of
//! committing to git, and nothing clears `txn` when a connection drops.
//! `docs/src/technical/server.md:51` claims the server path cannot span BEGIN
//! across calls.
//!
//! The rule pinned here: one client's open transaction must never capture
//! another client's write, and a client vanishing mid-transaction must not
//! wedge the server. A failure naming `buffered-into-foreign-txn` means a
//! GraphQL INSERT reported success while its doogat never reached git HEAD.
//! One naming `wedged-after-disconnect` means the server stayed inside the
//! dead PgWire client's transaction.

use crate::common::{DdbTestRepo, ServerGuard};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Every path in the repo's git HEAD tree (committed state, not the work tree).
fn head_paths(repo: &Path) -> Vec<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .output()
        .expect("git ls-tree failed to run");
    assert!(
        out.status.success(),
        "git ls-tree HEAD failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Assert the doogat `id` is committed in HEAD and carries `title`.
/// `leg` names which hazard leg fired when the assertion fails.
fn assert_committed(repo: &Path, id: &str, title: &str, leg: &str) {
    let paths = head_paths(repo);
    let path = paths.iter().find(|p| p.contains(id)).unwrap_or_else(|| {
        panic!(
            "{leg}: INSERT of '{title}' returned id {id} as success, but no file for it exists in git HEAD; committed paths: {paths:?}"
        )
    });
    let out = Command::new("git")
        .current_dir(repo)
        .args(["show", &format!("HEAD:{path}")])
        .output()
        .expect("git show failed to run");
    assert!(
        out.status.success(),
        "{leg}: git show HEAD:{path} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = String::from_utf8_lossy(&out.stdout);
    assert!(
        content.contains(title),
        "{leg}: committed doogat {id} does not carry title '{title}': {content}"
    );
}

/// Run one INSERT through GraphQL `executeSql` and return the new doogat id.
fn insert_probe(server: &ServerGuard, title: &str, leg: &str) -> String {
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": format!("INSERT INTO leakprobe (title) VALUES ('{title}')") }),
    );
    assert!(
        result.get("errors").is_none(),
        "{leg}: GraphQL INSERT of '{title}' failed: {result}"
    );
    result["data"]["executeSql"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("{leg}: INSERT of '{title}' returned no id: {result}"))
        .to_string()
}

#[test]
#[ignore = "fast-track FT-6: hazard H6 confirmed 2026-09-06 (server transaction buffer is process-global; a foreign PgWire BEGIN swallows a GraphQL INSERT); un-ignore with the fix, see dev/local/plans/fast-track-2026-09-06.md"]
fn open_transaction_on_one_connection_does_not_capture_another_clients_insert() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE leakprobe (title TEXT NOT NULL)" }),
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE leakprobe failed: {create}"
    );

    // A PgWire client opens a transaction and never commits it.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = rt.block_on(async {
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
    });
    rt.block_on(client.simple_query("BEGIN"))
        .expect("pgwire BEGIN should succeed");

    // Leg 1: a different client's INSERT must reach git, not the PgWire
    // client's transaction buffer.
    let leaked_id = insert_probe(&server, "leak-probe", "buffered-into-foreign-txn");
    assert_committed(
        repo.path(),
        &leaked_id,
        "leak-probe",
        "buffered-into-foreign-txn",
    );

    // The PgWire client vanishes without COMMIT or ROLLBACK.
    drop(client);
    // Let the server observe the disconnect, and land the next id in a later
    // second so it cannot collide with the first probe's id.
    std::thread::sleep(Duration::from_millis(1200));

    // Leg 2: the server must not still be inside the dead client's transaction.
    let post_drop_id = insert_probe(&server, "post-drop-probe", "wedged-after-disconnect");
    assert_committed(
        repo.path(),
        &post_drop_id,
        "post-drop-probe",
        "wedged-after-disconnect",
    );

    let read = server.graphql(r#"{ doogats { id title } }"#);
    assert!(
        read.get("errors").is_none(),
        "wedged-after-disconnect: plain GraphQL read failed after a PgWire client disappeared mid-transaction: {read}"
    );
}
