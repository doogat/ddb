use crate::common::{ServerGuard, DdbTestRepo};
use std::sync::Arc;

#[test]
fn compact_mutation_returns_result() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server
        .graphql(r#"mutation { compact { filesRemoved crdtDocsCompacted gcSuccess backupPath } }"#);
    assert!(result.get("errors").is_none(), "compact failed: {result}");
    let compact = &result["data"]["compact"];
    assert!(compact["filesRemoved"].is_i64());
    assert!(compact["crdtDocsCompacted"].is_i64());
    assert!(compact["gcSuccess"].is_boolean());
}

#[test]
fn compact_force_mutation() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(
        r#"mutation { compact(force: true) { filesRemoved crdtDocsCompacted gcSuccess backupPath } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "compact(force: true) failed: {result}"
    );
}

#[test]
fn compact_with_node_produces_backup() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "TestNode"])
        .assert()
        .success();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(r#"mutation { compact(force: true) { gcSuccess backupPath } }"#);
    assert!(
        result.get("errors").is_none(),
        "compact with node failed: {result}"
    );
    let compact = &result["data"]["compact"];
    assert!(
        compact["backupPath"].is_string(),
        "compact with registered node should produce backupPath: {result}"
    );
}

#[test]
fn compact_no_backup_mutation() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "TestNode"])
        .assert()
        .success();
    let server = ServerGuard::start(&repo);

    let result = server
        .graphql(r#"mutation { compact(force: true, noBackup: true) { gcSuccess backupPath } }"#);
    assert!(
        result.get("errors").is_none(),
        "compact(noBackup: true) failed: {result}"
    );
    let compact = &result["data"]["compact"];
    assert!(
        compact["backupPath"].is_null(),
        "compact(noBackup: true) should have null backupPath: {result}"
    );
}

#[test]
fn sync_mutation_no_remote_returns_error() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // No remote configured — should return an error, not panic
    let result = server.graphql(
        r#"mutation { sync { direction commitsTransferred conflictsResolved resurrected } }"#,
    );
    assert!(
        result.get("errors").is_some(),
        "sync without remote should error: {result}"
    );
}

#[test]
fn sync_mutation_with_remote() {
    use tempfile::TempDir;

    // Set up a bare remote
    let remote_dir = TempDir::new().unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(remote_dir.path())
        .status()
        .expect("failed to spawn git init");
    assert!(status.success(), "git init --bare failed");

    let repo = DdbTestRepo::init();

    // Add remote + register node
    let status = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["remote", "add", "origin"])
        .arg(remote_dir.path())
        .status()
        .expect("failed to spawn git remote add");
    assert!(status.success(), "git remote add failed");
    repo.ddb()
        .args(["register-node", "TestNode"])
        .assert()
        .success();

    // Push initial state
    let status = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["push", "-u", "origin", "master"])
        .status()
        .expect("failed to spawn git push");
    assert!(status.success(), "git push failed");

    let server = ServerGuard::start(&repo);

    let result = server.graphql(
        r#"mutation { sync { direction commitsTransferred conflictsResolved resurrected } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "sync with remote failed: {result}"
    );
    let sync = &result["data"]["sync"];
    assert!(sync["direction"].is_string());
    assert!(sync["commitsTransferred"].is_i64());
    assert!(sync["conflictsResolved"].is_i64());
    assert!(sync["resurrected"].is_i64());
}

#[test]
fn sync_during_writes_serialized_through_actor() {
    use tempfile::TempDir;

    // Set up a bare remote
    let remote_dir = TempDir::new().unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "--bare"])
        .arg(remote_dir.path())
        .status()
        .expect("failed to spawn git init");
    assert!(status.success(), "git init --bare failed");

    let repo = DdbTestRepo::init();

    // Add remote + register node
    let status = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["remote", "add", "origin"])
        .arg(remote_dir.path())
        .status()
        .expect("failed to spawn git remote add");
    assert!(status.success(), "git remote add failed");
    repo.ddb()
        .args(["register-node", "TestNode"])
        .assert()
        .success();

    let status = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["push", "-u", "origin", "master"])
        .status()
        .expect("failed to spawn git push");
    assert!(status.success(), "git push failed");

    let server = Arc::new(ServerGuard::start(&repo));

    // Count commits before concurrent operations
    let pre_count = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("git rev-list failed");
    let commits_before: usize = String::from_utf8_lossy(&pre_count.stdout)
        .trim()
        .parse()
        .unwrap();

    // Spawn concurrent writers + sync, collecting created IDs
    let mut handles: Vec<std::thread::JoinHandle<Option<String>>> = Vec::new();

    for i in 0..5 {
        let srv = Arc::clone(&server);
        handles.push(std::thread::spawn(move || {
            let result = srv.graphql_with_vars(
                r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
                serde_json::json!({ "input": {
                    "title": format!("Concurrent Write {i}"),
                    "content": format!("body {i}"),
                } }),
            );
            assert!(
                result.get("errors").is_none(),
                "concurrent write {i} failed: {result}"
            );
            result
                .pointer("/data/createDoogat/id")
                .and_then(|v| v.as_str())
                .map(String::from)
        }));
    }

    // Sync concurrently with writes
    let srv = Arc::clone(&server);
    handles.push(std::thread::spawn(move || {
        let result = srv.graphql(
            r#"mutation { sync { direction commitsTransferred conflictsResolved resurrected } }"#,
        );
        assert!(
            result.get("errors").is_none(),
            "concurrent sync failed: {result}"
        );
        None // sync doesn't create a doogat
    }));

    // All must complete without panic or error
    let mut created_ids = Vec::new();
    for h in handles {
        let id = h
            .join()
            .expect("thread panicked during concurrent mutations");
        if let Some(id) = id {
            created_ids.push(id);
        }
    }

    // Verify serialization: all 5 doogats were created and are queryable
    assert_eq!(
        created_ids.len(),
        5,
        "expected 5 created IDs, got {}: {:?}",
        created_ids.len(),
        created_ids
    );

    for id in &created_ids {
        let query = format!(r#"{{ doogat(id: "{id}") {{ id title }} }}"#);
        let result = server.graphql(&query);
        assert!(
            result.get("errors").is_none(),
            "doogat {id} not found after concurrent writes: {result}"
        );
        assert_eq!(
            result.pointer("/data/doogat/id").and_then(|v| v.as_str()),
            Some(id.as_str()),
            "doogat {id} returned wrong data: {result}"
        );
    }

    // Verify serialization: each create produced a distinct commit
    let post_count = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("git rev-list failed");
    let commits_after: usize = String::from_utf8_lossy(&post_count.stdout)
        .trim()
        .parse()
        .unwrap();
    let new_commits = commits_after - commits_before;
    assert!(
        new_commits >= 5,
        "expected at least 5 new commits (one per create), got {new_commits}"
    );
}
