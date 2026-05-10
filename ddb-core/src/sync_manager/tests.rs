use super::*;

fn temp_repo() -> (tempfile::TempDir, GitRepo) {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    (dir, repo)
}

#[test]
fn register_and_open_node() {
    let (_dir, repo) = temp_repo();
    let node = register_node(&repo, "Laptop").unwrap();
    assert!(!node.uuid.is_empty());
    assert_eq!(node.name, "Laptop");

    // Should be able to open
    let mgr = SyncManager::open(&repo).unwrap();
    assert_eq!(mgr.node.uuid, node.uuid);
}

#[test]
fn list_nodes() {
    let (_dir, repo) = temp_repo();
    register_node(&repo, "Laptop").unwrap();

    let mgr = SyncManager::open(&repo).unwrap();
    let nodes = mgr.list_nodes().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "Laptop");
}

#[test]
fn open_without_registration_fails() {
    let (_dir, repo) = temp_repo();
    let result = SyncManager::open(&repo);
    assert!(result.is_err());
}

#[test]
fn sync_state_update() {
    let (_dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();
    let mut mgr = SyncManager::open(&repo).unwrap();

    mgr.update_sync_state().unwrap();
    assert!(!mgr.node.known_heads.is_empty());
    assert!(mgr.node.last_sync.is_some());
}

#[test]
fn node_status_defaults_to_active() {
    let (_dir, repo) = temp_repo();
    let node = register_node(&repo, "Test").unwrap();
    assert_eq!(node.status, crate::types::NodeStatus::Active);
    assert!(node.created.is_some());
}

#[test]
fn retire_and_list_nodes() {
    let (_dir, repo) = temp_repo();
    register_node(&repo, "Laptop").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let nodes = mgr.list_nodes().unwrap();
    assert_eq!(nodes[0].status, crate::types::NodeStatus::Active);

    mgr.retire_node(&nodes[0].uuid).unwrap();
    let nodes = mgr.list_nodes().unwrap();
    assert_eq!(nodes[0].status, crate::types::NodeStatus::Retired);
}

#[test]
fn backward_compat_old_toml_without_status() {
    let (_dir, repo) = temp_repo();
    // Write an old-style node config without status/created fields
    let uuid = "test-uuid-1234";
    let old_toml = format!("uuid = \"{uuid}\"\nname = \"OldNode\"\nknown_heads = []\n");
    repo.commit_file(&format!(".nodes/{uuid}.toml"), &old_toml, "old node")
        .unwrap();
    std::fs::write(repo.path.join(".git/ddb-node"), uuid).unwrap();

    let mgr = SyncManager::open(&repo).unwrap();
    assert_eq!(mgr.node.status, crate::types::NodeStatus::Active); // default
}

#[test]
fn resurrected_marker_added() {
    let content = "---\ntitle: Test\n---\nBody content.";
    let result = add_resurrected_marker(content);
    assert!(result.contains("resurrected: true"));
    assert!(result.contains("title: Test"));
    assert!(result.contains("Body content"));
}

#[test]
fn resurrected_marker_not_duplicated() {
    let content = "---\ntitle: Test\nresurrected: true\n---\nBody.";
    let result = add_resurrected_marker(content);
    assert_eq!(result.matches("resurrected").count(), 1);
}

#[test]
fn clean_merge_validation_falls_back_to_crdt() {
    let (dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();

    let path = "ddb/note.md";
    let ancestor = "---\ntitle: Base\n---\nBody base.\n---\n- source:: base";
    let ours = "---\ntitle: Ours\n---\nBody ours.\n---\n- source:: base";
    let theirs = "---\ntitle: Base\n---\nBody base.\n---\n- source:: theirs";
    let merged_invalid = "---\ntitle: Broken\n---\nsource:: body\n---\n- source:: ref";

    repo.commit_file(path, ancestor, "ancestor").unwrap();
    let ancestor_hash = repo.head_oid().unwrap();
    repo.commit_file(path, ours, "ours edit").unwrap();
    let ours_hash = repo.head_oid().unwrap();

    let ancestor_commit = repo
        .repo
        .find_commit(git2::Oid::from_str(&ancestor_hash.0).unwrap())
        .unwrap();
    repo.repo.branch("theirs", &ancestor_commit, true).unwrap();
    repo.repo.set_head("refs/heads/theirs").unwrap();
    repo.repo
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    repo.commit_file(path, theirs, "theirs edit").unwrap();
    let theirs_hash = repo.head_oid().unwrap();

    repo.repo.set_head("refs/heads/master").unwrap();
    repo.repo
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    assert_eq!(repo.head_oid().unwrap(), ours_hash);

    let merge_hash = repo
        .commit_merge(
            &[(path, merged_invalid)],
            &[],
            "synthetic clean merge",
            &theirs_hash,
        )
        .unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index = crate::indexer::Index::open(&db_path).unwrap();
    let mgr = SyncManager::open(&repo).unwrap();
    let resolved = mgr
        .validate_clean_merge_or_fallback(merge_hash, &index)
        .unwrap();
    assert_eq!(resolved, 1);

    let repaired = repo.read_file(path).unwrap();
    assert!(parser::parse(&repaired, path).is_ok());
    let head = repo.repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 1);
}

#[test]
fn lww_fallback_when_crdt_produces_invalid_output() {
    // Test the cascade: CRDT resolve -> validation fails -> LWW fallback.
    // We set up a real git merge conflict where the CRDT merge produces
    // invalid output (malformed frontmatter), triggering LWW fallback.
    let (dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();

    let path = "ddb/20260301120000.md";

    // Ancestor: valid doogat
    let ancestor = "---\nid: 20260301120000\ntitle: Base\ndate: 2026-03-01\n---\nBase body.\n---\n- source:: base";
    repo.commit_file(path, ancestor, "ancestor").unwrap();
    let ancestor_hash = repo.head_oid().unwrap();

    // Ours: valid edit
    let ours = "---\nid: 20260301120000\ntitle: Ours Edit\ndate: 2026-03-01\n---\nOurs body.\n---\n- source:: ours";
    repo.commit_file(path, ours, "ours edit").unwrap();

    // Create theirs branch from ancestor
    let ancestor_commit = repo
        .repo
        .find_commit(git2::Oid::from_str(&ancestor_hash.0).unwrap())
        .unwrap();
    repo.repo
        .branch("theirs_lww", &ancestor_commit, true)
        .unwrap();
    repo.repo.set_head("refs/heads/theirs_lww").unwrap();
    repo.repo
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    // Theirs: valid edit with cross-zone inline field duplication
    let theirs = "---\nid: 20260301120000\ntitle: Theirs Edit\ndate: 2026-03-01\n---\nsource:: theirs_body_field\n---\n- source:: theirs";
    repo.commit_file(path, theirs, "theirs edit").unwrap();
    let theirs_hash = repo.head_oid().unwrap();

    // Switch back to master
    repo.repo.set_head("refs/heads/master").unwrap();
    repo.repo
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    // Create a synthetic merge commit with INVALID content
    // (malformed YAML that parser::parse will reject)
    let merged_invalid = "---\ntitle: Broken\n: invalid yaml [\n---\nBody.";
    let merge_hash = repo
        .commit_merge(
            &[(path, merged_invalid)],
            &[],
            "synthetic merge with invalid content",
            &theirs_hash,
        )
        .unwrap();

    // validate_clean_merge_or_fallback detects the invalid content,
    // builds ConflictFile from the two parent commits, and calls
    // cascade_resolve. The CRDT merge of ours+theirs may also produce
    // a cross-zone "source" duplicate (body + ref) which fails validation,
    // triggering LWW fallback.
    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index = crate::indexer::Index::open(&db_path).unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let resolved_count = mgr
        .validate_clean_merge_or_fallback(merge_hash, &index)
        .unwrap();
    assert_eq!(resolved_count, 1, "should have resolved 1 conflict");

    // The key assertion: the resolved file MUST be valid (parseable).
    // Whether CRDT or LWW won, the cascade guarantees a valid result.
    let final_content = repo.read_file(path).unwrap();
    let parsed = crate::parser::parse(&final_content, path);
    assert!(
        parsed.is_ok(),
        "cascade_resolve must produce parseable output: {:?}",
        parsed.err()
    );

    // The resolved title must be from one of the two parents (not the invalid merge)
    let parsed = parsed.unwrap();
    let title = parsed.meta.title.as_deref().unwrap_or("");
    assert!(
        title == "Ours Edit" || title == "Theirs Edit",
        "title should be from one of the parents, got: {title}"
    );
}

#[test]
fn sync_error_resets_skip_commit_graph_for_subsequent_commits() {
    let (dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();

    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index = crate::indexer::Index::open(&db_path).unwrap();

    let mut mgr = SyncManager::open(&repo).unwrap();
    let sync_result = mgr.sync("missing-remote", "master", &index);
    assert!(sync_result.is_err());

    let commit_graph = repo.path.join(".git/objects/info/commit-graph");
    let _ = std::fs::remove_file(&commit_graph);
    assert!(!commit_graph.exists());

    repo.commit_file(
        "ddb/after-sync-error.md",
        "---\ntitle: After sync error\n---\nBody",
        "post-sync-error commit",
    )
    .unwrap();

    assert!(commit_graph.exists());
}

#[test]
fn add_add_collision_detected() {
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nBody ours\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nBody theirs\n".into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    };
    assert!(conflict.ancestor.is_none());
    assert!(!conflict.ours.is_empty());
    assert!(!conflict.theirs.is_empty());

    let (winner, loser) = resolve_add_add_collision(&conflict);
    assert_eq!(winner.path, conflict.path);
    assert!(!loser.content.is_empty());
    assert_eq!(loser.old_id, "20260101120000");
}

#[test]
fn add_add_winner_by_hlc() {
    let earlier = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "nodeA".into(),
    };
    let later = crate::hlc::Hlc {
        wall_ms: 2000,
        counter: 0,
        node: "nodeB".into(),
    };
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n".into(),
        ours_hlc: Some(earlier),
        theirs_hlc: Some(later),
        ours_blob_oid: None,
        theirs_blob_oid: None,
    };
    let (winner, loser) = resolve_add_add_collision(&conflict);
    // Theirs has later HLC, so theirs wins
    assert!(winner.content.contains("Theirs body"));
    assert!(loser.content.contains("Ours body"));
}

#[test]
fn add_add_hlc_tie_theirs_wins() {
    // Both HLCs None → theirs wins
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n".into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: None,
        theirs_blob_oid: None,
    };
    let (winner, _loser) = resolve_add_add_collision(&conflict);
    assert!(winner.content.contains("Theirs body"));

    // Equal HLCs → theirs wins
    let hlc = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "nodeA".into(),
    };
    let conflict2 = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs\n".into(),
        ours_hlc: Some(hlc.clone()),
        theirs_hlc: Some(hlc),
        ours_blob_oid: None,
        theirs_blob_oid: None,
    };
    let (winner2, _) = resolve_add_add_collision(&conflict2);
    assert!(winner2.content.contains("Theirs"));
}

#[test]
fn update_frontmatter_id_replaces_id() {
    let content = "---\nid: 20260101120000\ntitle: Test\n---\nBody\n";
    let updated = update_frontmatter_id(content, "20260301120000").unwrap();
    assert!(updated.contains("id: 20260301120000"));
    assert!(!updated.contains("id: 20260101120000"));
    assert!(updated.contains("title: Test"));
    assert!(updated.contains("Body"));
}

#[test]
fn add_add_full_sync_both_survive() {
    // Two repos, bare remote, same-ID collision, both survive after sync.
    let bare_dir = tempfile::TempDir::new().unwrap();
    git2::Repository::init_bare(bare_dir.path()).unwrap();

    let (dir_a, repo_a) = temp_repo();
    repo_a
        .add_remote("origin", bare_dir.path().to_str().unwrap())
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    register_node(&repo_a, "A").unwrap();
    repo_a.push("origin", "master").unwrap();

    let dir_b = tempfile::TempDir::new().unwrap();
    git2::Repository::clone(bare_dir.path().to_str().unwrap(), dir_b.path()).unwrap();
    let repo_b = GitRepo::open(dir_b.path()).unwrap();
    register_node(&repo_b, "B").unwrap();
    repo_b.push("origin", "master").unwrap();

    // Sync A to get B's node
    repo_a.fetch("origin", "master").unwrap();
    repo_a.merge_remote("origin", "master").unwrap();

    // Both create the same-ID doogat
    let id = "20260101120000";
    repo_a
        .commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\nid: {id}\ntitle: From A\n---\nA body\n"),
            "A creates",
        )
        .unwrap();
    repo_b
        .commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\nid: {id}\ntitle: From B\n---\nB body\n"),
            "B creates",
        )
        .unwrap();

    // A pushes, B syncs
    repo_a.push("origin", "master").unwrap();
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert_eq!(report.collisions_reassigned, 1);

    // Both doogats should exist with distinct IDs
    let zk = dir_b.path().join("ddb");
    let files: Vec<String> = std::fs::read_dir(&zk)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !name.starts_with('_') {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(files.len(), 2, "both doogats should survive: {files:?}");

    // Verify convergence: push B, sync A
    repo_b.push("origin", "master").unwrap();
    let db_a = dir_a.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_a.parent().unwrap()).unwrap();
    let index_a = crate::indexer::Index::open(&db_a).unwrap();
    let mut mgr_a = SyncManager::open(&repo_a).unwrap();
    mgr_a.sync("origin", "master", &index_a).unwrap();

    let files_a: Vec<String> = std::fs::read_dir(dir_a.path().join("ddb"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !name.starts_with('_') {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    let mut sorted_a = files_a;
    sorted_a.sort();
    let mut sorted_b = files;
    sorted_b.sort();
    assert_eq!(sorted_a, sorted_b, "nodes should converge");
}

/// Helper: set up two repos (A, B) with a shared bare remote, both registered
/// as sync nodes. Returns (dir_a, repo_a, dir_b, repo_b, bare_dir).
fn setup_binary_sync_pair() -> (
    tempfile::TempDir,
    GitRepo,
    tempfile::TempDir,
    GitRepo,
    tempfile::TempDir,
) {
    let bare_dir = tempfile::TempDir::new().unwrap();
    git2::Repository::init_bare(bare_dir.path()).unwrap();

    let (dir_a, repo_a) = temp_repo();
    repo_a
        .add_remote("origin", bare_dir.path().to_str().unwrap())
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    register_node(&repo_a, "A").unwrap();
    repo_a.push("origin", "master").unwrap();

    let dir_b = tempfile::TempDir::new().unwrap();
    git2::Repository::clone(bare_dir.path().to_str().unwrap(), dir_b.path()).unwrap();
    let repo_b = GitRepo::open(dir_b.path()).unwrap();
    register_node(&repo_b, "B").unwrap();
    repo_b.push("origin", "master").unwrap();

    // Sync A to pick up B's node registration
    repo_a.fetch("origin", "master").unwrap();
    repo_a.merge_remote("origin", "master").unwrap();

    (dir_a, repo_a, dir_b, repo_b, bare_dir)
}

#[test]
fn binary_lww_ours_wins_higher_hlc() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    let bin_path = "reference/test/photo.bin";
    let ours_bytes = b"OURS_BINARY_CONTENT_AAA";
    let theirs_bytes = b"THEIRS_BINARY_CONTENT_BBB";

    // A commits with a LOWER HLC (earlier timestamp)
    let hlc_a = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let msg_a = crate::hlc::append_hlc_trailer("A adds binary", &hlc_a);
    repo_a
        .commit_binary_file(bin_path, theirs_bytes, &msg_a)
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B commits with a HIGHER HLC (later timestamp) — this is "ours" from B's perspective
    let hlc_b = crate::hlc::Hlc {
        wall_ms: 9000,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    let msg_b = crate::hlc::append_hlc_trailer("B adds binary", &hlc_b);
    repo_b
        .commit_binary_file(bin_path, ours_bytes, &msg_b)
        .unwrap();

    // B syncs — ours (B, wall_ms=9000) > theirs (A, wall_ms=1000), ours wins
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert!(
        report.conflicts_resolved > 0,
        "should have resolved a conflict"
    );
    let resolved = std::fs::read(dir_b.path().join(bin_path)).unwrap();
    assert_eq!(resolved, ours_bytes, "ours (higher HLC) should win");
}

#[test]
fn binary_lww_theirs_wins_higher_hlc() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    let bin_path = "reference/test/photo.bin";
    let ours_bytes = b"OURS_BINARY_CONTENT_AAA";
    let theirs_bytes = b"THEIRS_BINARY_CONTENT_BBB";

    // A commits with a HIGHER HLC — this will be "theirs" from B's perspective
    let hlc_a = crate::hlc::Hlc {
        wall_ms: 9000,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let msg_a = crate::hlc::append_hlc_trailer("A adds binary", &hlc_a);
    repo_a
        .commit_binary_file(bin_path, theirs_bytes, &msg_a)
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B commits with a LOWER HLC
    let hlc_b = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    let msg_b = crate::hlc::append_hlc_trailer("B adds binary", &hlc_b);
    repo_b
        .commit_binary_file(bin_path, ours_bytes, &msg_b)
        .unwrap();

    // B syncs — theirs (A, wall_ms=9000) > ours (B, wall_ms=1000), theirs wins
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert!(
        report.conflicts_resolved > 0,
        "should have resolved a conflict"
    );
    let resolved = std::fs::read(dir_b.path().join(bin_path)).unwrap();
    assert_eq!(resolved, theirs_bytes, "theirs (higher HLC) should win");
}

#[test]
fn binary_lww_theirs_wins_on_tie() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    let bin_path = "reference/test/photo.bin";
    let ours_bytes = b"OURS_TIE_CONTENT";
    let theirs_bytes = b"THEIRS_TIE_CONTENT";

    // Both use identical HLC timestamps
    let hlc = crate::hlc::Hlc {
        wall_ms: 5000,
        counter: 0,
        node: "aaaaaaaa".into(),
    };

    let msg_a = crate::hlc::append_hlc_trailer("A adds binary", &hlc);
    repo_a
        .commit_binary_file(bin_path, theirs_bytes, &msg_a)
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    let msg_b = crate::hlc::append_hlc_trailer("B adds binary", &hlc);
    repo_b
        .commit_binary_file(bin_path, ours_bytes, &msg_b)
        .unwrap();

    // B syncs — tied HLC, theirs wins by convention
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert!(
        report.conflicts_resolved > 0,
        "should have resolved a conflict"
    );
    let resolved = std::fs::read(dir_b.path().join(bin_path)).unwrap();
    assert_eq!(resolved, theirs_bytes, "theirs should win on HLC tie");
}

#[test]
fn binary_lww_theirs_wins_missing_hlc() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    let bin_path = "reference/test/photo.bin";
    let ours_bytes = b"OURS_NO_HLC";
    let theirs_bytes = b"THEIRS_NO_HLC";

    // Neither commit includes an HLC trailer
    repo_a
        .commit_binary_file(bin_path, theirs_bytes, "A adds binary (no HLC)")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_binary_file(bin_path, ours_bytes, "B adds binary (no HLC)")
        .unwrap();

    // B syncs — no HLC on either side, theirs wins by convention
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert!(
        report.conflicts_resolved > 0,
        "should have resolved a conflict"
    );
    let resolved = std::fs::read(dir_b.path().join(bin_path)).unwrap();
    assert_eq!(
        resolved, theirs_bytes,
        "theirs should win when both HLCs missing"
    );
}

#[test]
fn binary_lww_preserves_exact_bytes() {
    // Use bytes that would be corrupted by String::from_utf8_lossy
    // (0xFF, 0xFE are invalid UTF-8 lead bytes; 0x00 is a null byte)
    let non_utf8: Vec<u8> = vec![
        0xFF, 0xFE, 0x00, 0x01, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xC1, 0xFD, 0xFE, 0xFF,
    ];

    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    let bin_path = "reference/test/corrupt-if-lossy.bin";

    // A commits the non-UTF8 content with a HIGHER HLC (theirs wins)
    let hlc_a = crate::hlc::Hlc {
        wall_ms: 9000,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let msg_a = crate::hlc::append_hlc_trailer("A adds non-utf8 binary", &hlc_a);
    repo_a
        .commit_binary_file(bin_path, &non_utf8, &msg_a)
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B commits different content with lower HLC
    let hlc_b = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    let msg_b = crate::hlc::append_hlc_trailer("B adds placeholder", &hlc_b);
    repo_b
        .commit_binary_file(bin_path, b"placeholder", &msg_b)
        .unwrap();

    // B syncs — theirs (A) wins, content must be byte-exact
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert!(
        report.conflicts_resolved > 0,
        "should have resolved a conflict"
    );
    let resolved = std::fs::read(dir_b.path().join(bin_path)).unwrap();
    assert_eq!(
        resolved, non_utf8,
        "binary content must survive without UTF-8 lossy corruption"
    );
    // Double-check: if lossy conversion had occurred, replacement char (0xEF 0xBF 0xBD)
    // would appear and lengths would differ
    assert_eq!(
        resolved.len(),
        non_utf8.len(),
        "byte length must match exactly"
    );
}

#[test]
fn binary_ref_delete_vs_edit_uses_resurrection() {
    // Delete-vs-edit on a reference/ path should go through the delete_edit
    // bucket (resurrection), NOT the binary_ref bucket (LWW).
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    let bin_path = "reference/test/photo.bin";
    let content = b"SOME_BINARY_CONTENT";

    // Both nodes start with the same file
    repo_a
        .commit_binary_file(bin_path, content, "add binary")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    repo_b.merge_remote("origin", "master").unwrap();

    // Node A: delete the binary file
    repo_a.delete_file(bin_path, "delete binary").unwrap();
    repo_a.push("origin", "master").unwrap();

    // Node B: modify the binary file (edit wins in delete-vs-edit)
    let new_content = b"MODIFIED_BINARY";
    repo_b
        .commit_binary_file(bin_path, new_content, "modify binary")
        .unwrap();

    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert!(
        report.conflicts_resolved > 0,
        "should have resolved a conflict"
    );
    // In delete-vs-edit, edit wins. The file should still exist.
    assert!(
        dir_b.path().join(bin_path).exists(),
        "reference/ file should survive delete-vs-edit (edit wins)"
    );
    assert_eq!(
        report.resurrected, 1,
        "should count as resurrected (delete-vs-edit), not binary LWW"
    );
}
