use crate::common::{DdbTestRepo, ServerGuard};
use std::sync::Arc;

#[test]
fn compact_mutation_returns_result() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server
        .graphql(r#"mutation { compact { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter crdtTempFilesBefore crdtTempFilesAfter repoBytesBefore repoBytesAfter backupPath } }"#);
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
        r#"mutation { compact(force: true) { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter repoBytesBefore repoBytesAfter backupPath } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "compact(force: true) failed: {result}"
    );
    let compact = &result["data"]["compact"];
    assert!(
        compact["gcSuccess"].is_boolean(),
        "missing gcSuccess: {result}"
    );
    assert!(
        compact.get("backupPath").is_some(),
        "missing backupPath: {result}"
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
        compact["gcSuccess"].is_boolean(),
        "missing gcSuccess: {result}"
    );
    assert!(
        compact["backupPath"].is_null(),
        "compact(noBackup: true) should have null backupPath: {result}"
    );
    assert!(
        compact.get("backupPath").is_some(),
        "missing backupPath: {result}"
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

fn commit_count(path: &std::path::Path) -> usize {
    let out = std::process::Command::new("git")
        .current_dir(path)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("git rev-list failed");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[test]
fn batch_update_via_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create 3 doogats
    let mut ids = Vec::new();
    for i in 1..=3 {
        let result = server.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": { "title": format!("Original {i}") } }),
        );
        assert!(
            result.get("errors").is_none(),
            "create {i} failed: {result}"
        );
        ids.push(
            result["data"]["createDoogat"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }

    // Batch update all 3 titles
    let query = format!(
        r#"mutation {{ batchUpdate(updates: [
            {{id: "{}", title: "Updated 1"}},
            {{id: "{}", title: "Updated 2"}},
            {{id: "{}", title: "Updated 3"}}
        ]) {{ id title }} }}"#,
        ids[0], ids[1], ids[2]
    );
    let result = server.graphql(&query);
    assert!(
        result.get("errors").is_none(),
        "batchUpdate failed: {result}"
    );
    let updated = result["data"]["batchUpdate"].as_array().unwrap();
    assert_eq!(updated.len(), 3, "expected 3 results: {result}");
    for (i, item) in updated.iter().enumerate() {
        assert_eq!(item["id"].as_str().unwrap(), ids[i]);
        assert_eq!(
            item["title"].as_str().unwrap(),
            format!("Updated {}", i + 1)
        );
    }

    // Re-query each to confirm persistence
    for (i, id) in ids.iter().enumerate() {
        let result = server.graphql(&format!(r#"{{ doogat(id: "{id}") {{ id title }} }}"#));
        assert!(
            result.get("errors").is_none(),
            "re-query {id} failed: {result}"
        );
        assert_eq!(
            result["data"]["doogat"]["title"].as_str().unwrap(),
            format!("Updated {}", i + 1)
        );
    }
}

#[test]
fn batch_update_atomicity_via_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create 2 doogats
    let mut ids = Vec::new();
    for i in 1..=2 {
        let result = server.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": { "title": format!("Keep {i}") } }),
        );
        assert!(
            result.get("errors").is_none(),
            "create {i} failed: {result}"
        );
        ids.push(
            result["data"]["createDoogat"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }

    // Batch update with one bad ID
    let query = format!(
        r#"mutation {{ batchUpdate(updates: [
            {{id: "{}", title: "Changed"}},
            {{id: "99999999999999", title: "Ghost"}}
        ]) {{ id title }} }}"#,
        ids[0]
    );
    let result = server.graphql(&query);
    assert!(
        result.get("errors").is_some(),
        "batchUpdate with bad ID should error: {result}"
    );

    // Verify originals unchanged
    for (i, id) in ids.iter().enumerate() {
        let result = server.graphql(&format!(r#"{{ doogat(id: "{id}") {{ id title }} }}"#));
        assert!(
            result.get("errors").is_none(),
            "re-query {id} failed: {result}"
        );
        assert_eq!(
            result["data"]["doogat"]["title"].as_str().unwrap(),
            format!("Keep {}", i + 1),
            "doogat {id} should be unchanged after failed batch"
        );
    }
}

#[test]
fn hyphenated_singleton_uses_snake_case_query_and_mutation_fields() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create_type = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": r#"CREATE TABLE "foo-bar" (title TEXT DEFAULT 'Foo Config', theme TEXT) SINGLETON"# }),
    );
    assert!(
        create_type.get("errors").is_none(),
        "CREATE TABLE foo-bar SINGLETON failed: {create_type}"
    );

    let seed_row = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": r#"INSERT INTO "foo-bar" (title, theme) VALUES ('Foo Config', 'dark')"# }),
    );
    assert!(
        seed_row.get("errors").is_none(),
        "singleton seed insert failed: {seed_row}"
    );

    let query = server.graphql(r#"{ foo_bar { id theme } }"#);
    assert!(
        query.get("errors").is_none(),
        "foo_bar query failed: {query}"
    );
    let id = query["data"]["foo_bar"]["id"]
        .as_str()
        .expect("singleton query should return id")
        .to_string();
    assert_eq!(query["data"]["foo_bar"]["theme"].as_str(), Some("dark"));

    let update = server.graphql(
        r#"mutation {
            update_foo_bar(input: "{\"theme\":\"light\"}") {
                id
                theme
            }
        }"#,
    );
    assert!(
        update.get("errors").is_none(),
        "update_foo_bar failed: {update}"
    );
    assert_eq!(
        update["data"]["update_foo_bar"]["id"].as_str(),
        Some(id.as_str()),
        "update_foo_bar should target the singleton row: {update}"
    );
    assert_eq!(
        update["data"]["update_foo_bar"]["theme"].as_str(),
        Some("light"),
        "update_foo_bar should update the singleton row: {update}"
    );

    let second_upsert = server.graphql(
        r#"mutation {
            upsert_foo_bar(input: "{\"theme\":\"blue\"}") {
                id
                created
            }
        }"#,
    );
    assert!(
        second_upsert.get("errors").is_none(),
        "upsert_foo_bar failed: {second_upsert}"
    );
    assert_eq!(
        second_upsert["data"]["upsert_foo_bar"]["id"].as_str(),
        Some(id.as_str()),
        "upsert should reuse the singleton row id: {second_upsert}"
    );
    assert_eq!(
        second_upsert["data"]["upsert_foo_bar"]["created"].as_bool(),
        Some(false),
        "upsert should update the existing singleton row: {second_upsert}"
    );

    let final_query = server.graphql(r#"{ foo_bar { id theme } }"#);
    assert!(
        final_query.get("errors").is_none(),
        "final foo_bar query failed: {final_query}"
    );
    assert_eq!(
        final_query["data"]["foo_bar"]["id"].as_str(),
        Some(id.as_str()),
        "final foo_bar query should still resolve the same singleton row: {final_query}"
    );
    assert_eq!(
        final_query["data"]["foo_bar"]["theme"].as_str(),
        Some("blue"),
        "final foo_bar query should reflect the last upsert: {final_query}"
    );
}

#[test]
fn batch_update_empty() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(r#"mutation { batchUpdate(updates: []) { id title } }"#);
    assert!(
        result.get("errors").is_none(),
        "batchUpdate([]) should succeed: {result}"
    );
    let updated = result["data"]["batchUpdate"].as_array().unwrap();
    assert!(updated.is_empty(), "expected empty array: {result}");
}

#[test]
fn batch_update_single_commit_via_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create 3 doogats
    let mut ids = Vec::new();
    for i in 1..=3 {
        let result = server.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": { "title": format!("Item {i}") } }),
        );
        assert!(
            result.get("errors").is_none(),
            "create {i} failed: {result}"
        );
        ids.push(
            result["data"]["createDoogat"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }

    let commits_before = commit_count(repo.path());

    // Batch update all 3
    let query = format!(
        r#"mutation {{ batchUpdate(updates: [
            {{id: "{}", title: "New 1"}},
            {{id: "{}", title: "New 2"}},
            {{id: "{}", title: "New 3"}}
        ]) {{ id }} }}"#,
        ids[0], ids[1], ids[2]
    );
    let result = server.graphql(&query);
    assert!(
        result.get("errors").is_none(),
        "batchUpdate failed: {result}"
    );

    let commits_after = commit_count(repo.path());
    assert_eq!(
        commits_after - commits_before,
        1,
        "batchUpdate should produce exactly 1 commit, got {}",
        commits_after - commits_before
    );
}

#[test]
fn create_many_basic() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(
        r#"mutation { createMany(inputs: [
            {title: "Bulk A"},
            {title: "Bulk B"},
            {title: "Bulk C"}
        ]) { id title } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "createMany failed: {result}"
    );
    let created = result["data"]["createMany"].as_array().unwrap();
    assert_eq!(created.len(), 3, "expected 3 results: {result}");
    assert_eq!(created[0]["title"].as_str().unwrap(), "Bulk A");
    assert_eq!(created[1]["title"].as_str().unwrap(), "Bulk B");
    assert_eq!(created[2]["title"].as_str().unwrap(), "Bulk C");

    let ids: Vec<&str> = created.iter().map(|c| c["id"].as_str().unwrap()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 3, "all IDs must be distinct");

    for item in created {
        let id = item["id"].as_str().unwrap();
        let q = format!(r#"{{ doogat(id: "{id}") {{ id title }} }}"#);
        let r = server.graphql(&q);
        assert!(r.get("errors").is_none(), "re-query {id} failed: {r}");
        assert_eq!(r["data"]["doogat"]["title"], item["title"]);
    }
}

#[test]
fn create_many_single_commit() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let before = commit_count(repo.path());

    let result = server.graphql(
        r#"mutation { createMany(inputs: [
            {title: "Commit A"},
            {title: "Commit B"},
            {title: "Commit C"}
        ]) { id } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "createMany failed: {result}"
    );

    let after = commit_count(repo.path());
    assert_eq!(
        after - before,
        1,
        "createMany should produce exactly 1 commit, got {}",
        after - before
    );
}

#[test]
fn create_many_with_type_and_fields() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create a type via SQL
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE item (category TEXT, priority INTEGER)" }),
    );
    assert!(
        result.get("errors").is_none(),
        "CREATE TABLE failed: {result}"
    );

    let result = server.graphql(
        r#"mutation { createMany(inputs: [
            {title: "Item 1", type: "item", fields: "{\"category\":\"books\",\"priority\":\"1\"}"},
            {title: "Item 2", type: "item", fields: "{\"category\":\"music\",\"priority\":\"2\"}"}
        ]) { id title type } }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "createMany with fields failed: {result}"
    );
    let created = result["data"]["createMany"].as_array().unwrap();
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["type"].as_str().unwrap(), "item");
    assert_eq!(created[1]["type"].as_str().unwrap(), "item");
}

#[test]
fn create_many_empty() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(r#"mutation { createMany(inputs: []) { id } }"#);
    assert!(
        result.get("errors").is_none(),
        "createMany([]) should succeed: {result}"
    );
    let created = result["data"]["createMany"].as_array().unwrap();
    assert!(created.is_empty(), "expected empty array: {result}");
}
