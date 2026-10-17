//! Maintenance verbs (sync, compact, run_maintenance) run on a dedicated
//! worker thread, so a slow sync no longer stalls ordinary writes, and the
//! repo write lock (00162) still serializes maintenance and actor writes.

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::{json, Value};
use std::io::Read;
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// A remote that accepts the connection, reads the request, then never
/// answers. The server's sync blocks until libgit2's remote timeout (30 s
/// default) fires.
fn stalled_remote() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while matches!(sock.read(&mut buf), Ok(n) if n > 0) {}
            });
        }
    });
    url
}

fn post_graphql(server: &ServerGuard, query: &str, timeout: Duration) -> Value {
    reqwest::blocking::Client::new()
        .post(server.url())
        .header("Authorization", format!("Bearer {}", server.token))
        .json(&json!({ "query": query }))
        .timeout(timeout)
        .send()
        .expect("request failed")
        .json()
        .expect("invalid json")
}

fn create(server: &ServerGuard, title: &str) -> String {
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        json!({ "input": { "title": title } }),
    );
    assert!(result.get("errors").is_none(), "create failed: {result}");
    result["data"]["createDoogat"]["id"]
        .as_str()
        .expect("create missing id")
        .to_string()
}

fn assert_readable(server: &ServerGuard, id: &str, title: &str) {
    let result = server.graphql_with_vars(
        r#"query($id: ID!) { doogat(id: $id) { title } }"#,
        json!({ "id": id }),
    );
    assert_eq!(
        result["data"]["doogat"]["title"], title,
        "doogat {id} lost: {result}"
    );
}

#[test]
fn integration_create_completes_while_slow_sync_runs() {
    let repo = DdbTestRepo::init();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["remote", "add", "origin", &stalled_remote()])
        .status()
        .unwrap();
    assert!(status.success());
    let server = ServerGuard::start(&repo);

    let sync = std::thread::scope(|s| {
        let sync = s.spawn(|| {
            post_graphql(
                &server,
                "mutation { sync { direction } }",
                Duration::from_secs(90),
            )
        });
        // Let the sync reach the stalled remote before the create.
        std::thread::sleep(Duration::from_millis(500));
        assert!(!sync.is_finished(), "sync returned before stalling");

        let start = Instant::now();
        let id = create(&server, "During Sync");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "create waited {elapsed:?} behind the stalled sync"
        );
        assert!(!sync.is_finished(), "sync finished before the create");
        assert_readable(&server, &id, "During Sync");
        sync.join().unwrap()
    });
    assert!(sync.get("errors").is_some(), "stalled sync should fail: {sync}");
}

#[test]
fn integration_maintenance_and_actor_writes_both_land() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);
    let seeded = create(&server, "Seed");

    let ids = std::thread::scope(|s| {
        let compact = s.spawn(|| {
            post_graphql(
                &server,
                "mutation { compact(force: true, noBackup: true) { filesRemoved gcSuccess } }",
                Duration::from_secs(60),
            )
        });
        let ids: Vec<String> = (0..5)
            .map(|i| create(&server, &format!("Concurrent {i}")))
            .collect();
        let compact = compact.join().unwrap();
        assert!(compact.get("errors").is_none(), "compact failed: {compact}");
        ids
    });

    assert_readable(&server, &seeded, "Seed");
    for (i, id) in ids.iter().enumerate() {
        assert_readable(&server, id, &format!("Concurrent {i}"));
    }
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["log", "--oneline"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&log.stdout);
    assert_eq!(
        log.matches("create doogat").count(),
        ids.len() + 1,
        "a create commit is missing:\n{log}"
    );
}
