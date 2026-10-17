//! Maintenance verbs (sync, compact, run_maintenance) run on a dedicated
//! worker thread, so a slow sync no longer stalls ordinary writes, and the
//! repo write lock (00162) still serializes maintenance and actor writes.

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::{json, Value};
use std::io::Read;
use std::net::TcpListener;
use std::time::{Duration, Instant};
use tempfile::TempDir;

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
    assert_eq!(
        sync["errors"][0]["extensions"]["code"].as_str(),
        Some("REMOTE_TIMEOUT"),
        "expected a REMOTE_TIMEOUT error code: {sync}"
    );
}

/// Push the repo's current history to a bare remote, then make one
/// unrelated commit directly against that remote so a later `sync` must
/// fetch and merge (a real git write by the maintenance worker), not a
/// no-op. Each remote lives in its own `TempDir` (never a fixed sibling
/// path) so parallel test runs on this machine can't collide on the same
/// directory; the caller must keep both alive for as long as the remote
/// is needed.
fn diverge_remote(repo: &DdbTestRepo) -> (TempDir, TempDir) {
    let remote = TempDir::new().unwrap();
    let remote_dir = remote.path().join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "--bare", remote_dir.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["remote", "add", "origin", remote_dir.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["push", "origin", "HEAD:refs/heads/master"])
        .status()
        .unwrap();
    assert!(status.success());

    let clone = TempDir::new().unwrap();
    let clone_dir = clone.path().join("remote-clone");
    let status = std::process::Command::new("git")
        .args([
            "clone",
            remote_dir.to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(clone_dir.join("remote-file.txt"), "remote change\n").unwrap();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&clone_dir)
        .args(["add", "remote-file.txt"])
        .status()
        .unwrap();
    assert!(status.success());
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&clone_dir)
        .args([
            "-c",
            "user.email=test@test",
            "-c",
            "user.name=test",
            "commit",
            "-m",
            "remote change",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&clone_dir)
        .args(["push", "origin", "HEAD:refs/heads/master"])
        .status()
        .unwrap();
    assert!(status.success());

    (remote, clone)
}

#[test]
fn integration_maintenance_and_actor_writes_both_land() {
    let repo = DdbTestRepo::init();
    let _remote_temps = diverge_remote(&repo);
    let server = ServerGuard::start(&repo);
    let seeded = create(&server, "Seed");

    let ids = std::thread::scope(|s| {
        let sync = s.spawn(|| {
            post_graphql(
                &server,
                "mutation { sync { direction } }",
                Duration::from_secs(60),
            )
        });
        let ids: Vec<String> = (0..5)
            .map(|i| create(&server, &format!("Concurrent {i}")))
            .collect();
        let sync = sync.join().unwrap();
        assert!(sync.get("errors").is_none(), "sync failed: {sync}");
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
