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

    // Build the synthetic clean-merge commit directly, NOT via `commit_merge`:
    // ours and theirs edit disjoint zones, so `merge_commits` auto-merges them
    // cleanly (no conflict) — routing this through `commit_merge` would correctly
    // trip its divergence guard (the resolved set would not match the empty
    // re-run conflict set). Overlay the deliberately-invalid content at `path` to
    // simulate a bad auto-merge, then commit a real 2-parent merge for
    // `validate_clean_merge_or_fallback` to detect and repair.
    let ours_commit = repo
        .repo
        .find_commit(git2::Oid::from_str(&ours_hash.0).unwrap())
        .unwrap();
    let their_commit = repo
        .repo
        .find_commit(git2::Oid::from_str(&theirs_hash.0).unwrap())
        .unwrap();
    let mut merge_index = repo
        .repo
        .merge_commits(&ours_commit, &their_commit, None)
        .unwrap();
    assert!(
        !merge_index.has_conflicts(),
        "disjoint-zone edits must auto-merge cleanly"
    );
    let blob = repo.repo.blob(merged_invalid.as_bytes()).unwrap();
    merge_index
        .add(&git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: 0,
            id: blob,
            flags: 0,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        })
        .unwrap();
    let tree_oid = merge_index.write_tree_to(&repo.repo).unwrap();
    let tree = repo.repo.find_tree(tree_oid).unwrap();
    let sig = git2::Signature::now("ddb", "ddb@localhost").unwrap();
    let merge_oid = repo
        .repo
        .commit(
            Some("HEAD"),
            &sig,
            &sig,
            "synthetic clean merge",
            &tree,
            &[&ours_commit, &their_commit],
        )
        .unwrap();
    repo.repo
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    let merge_hash = crate::types::CommitHash(merge_oid.to_string());

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
        ours_blob_oid: Some("oid-ours-detected".into()),
        theirs_blob_oid: Some("oid-theirs-detected".into()),
    };
    assert!(conflict.ancestor.is_none());
    assert!(!conflict.ours.is_empty());
    assert!(!conflict.theirs.is_empty());

    let (winner, loser) = resolve_add_add_collision(&conflict).unwrap();
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
        ours_blob_oid: Some("oid-ours-winner".into()),
        theirs_blob_oid: Some("oid-theirs-winner".into()),
    };
    let (winner, loser) = resolve_add_add_collision(&conflict).unwrap();
    // Theirs has later HLC, so theirs wins
    assert!(winner.content.contains("Theirs body"));
    assert!(loser.content.contains("Ours body"));
}

#[test]
fn add_add_missing_or_equal_hlc_picks_higher_content_key() {
    // add-add's content key is the side's full text. "Zebra" text > "Apple"
    // text by str `>` (they diverge at the title line: 'Z' > 'A').
    let apple = "---\nid: 20260101120000\ntitle: Apple\n---\nApple body\n";
    let zebra = "---\nid: 20260101120000\ntitle: Zebra\n---\nZebra body\n";

    // Both HLCs None → content key decides → zebra (higher) wins.
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: apple.into(),
        theirs: zebra.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: Some("oid-ours-nohlc".into()),
        theirs_blob_oid: Some("oid-theirs-nohlc".into()),
    };
    let (winner, _loser) = resolve_add_add_collision(&conflict).unwrap();
    assert!(
        winner.content.contains("Zebra body"),
        "higher content key wins when both HLCs are missing"
    );

    // Exactly-equal HLCs → content key still decides → zebra wins.
    let hlc = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "nodeA".into(),
    };
    let conflict_eq = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: apple.into(),
        theirs: zebra.into(),
        ours_hlc: Some(hlc.clone()),
        theirs_hlc: Some(hlc),
        ours_blob_oid: Some("oid-ours-eqhlc".into()),
        theirs_blob_oid: Some("oid-theirs-eqhlc".into()),
    };
    let (winner_eq, _) = resolve_add_add_collision(&conflict_eq).unwrap();
    assert!(
        winner_eq.content.contains("Zebra body"),
        "higher content key wins when HLCs are exactly equal"
    );

    // Role-swap convergence: swap ours/theirs → same winning content.
    let conflict_swapped = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: zebra.into(),
        theirs: apple.into(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: Some("oid-ours-swaphlc".into()),
        theirs_blob_oid: Some("oid-theirs-swaphlc".into()),
    };
    let (winner_swapped, _) = resolve_add_add_collision(&conflict_swapped).unwrap();
    assert!(
        winner_swapped.content.contains("Zebra body"),
        "role swap converges on the same winning content"
    );
}

#[test]
fn add_add_loser_blob_oid_role_swap_converges() {
    // Two nodes independently observe the same collision with ours/theirs
    // reversed. Both must derive the same losing_blob_oid, since that value
    // feeds the new loser ID mint - if it diverged, the two nodes would mint
    // different IDs for the same losing content.
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
    let ours_oid = "oid-ours-swap";
    let theirs_oid = "oid-theirs-swap";
    let ours_body = "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n";
    let theirs_body = "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n";

    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: ours_body.into(),
        theirs: theirs_body.into(),
        ours_hlc: Some(earlier.clone()),
        theirs_hlc: Some(later.clone()),
        ours_blob_oid: Some(ours_oid.into()),
        theirs_blob_oid: Some(theirs_oid.into()),
    };
    let (_winner, loser) = resolve_add_add_collision(&conflict).unwrap();

    // Role-swapped: ours/theirs, their HLCs, and their blob OIDs all swapped
    // together, mirroring lww_pick_role_swap_converges_on_same_content_key.
    let conflict_swapped = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: theirs_body.into(),
        theirs: ours_body.into(),
        ours_hlc: Some(later),
        theirs_hlc: Some(earlier),
        ours_blob_oid: Some(theirs_oid.into()),
        theirs_blob_oid: Some(ours_oid.into()),
    };
    let (_winner_swapped, loser_swapped) = resolve_add_add_collision(&conflict_swapped).unwrap();

    assert_eq!(
        loser.losing_blob_oid, loser_swapped.losing_blob_oid,
        "two nodes observing the same collision with ours/theirs swapped must \
         derive the same losing_blob_oid, or they would mint different loser IDs \
         for the same content"
    );
}

#[test]
fn add_add_loser_blob_oid_is_ours_when_theirs_wins() {
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
    let ours_oid = "oid-ours-a";
    let theirs_oid = "oid-theirs-a";
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n".into(),
        ours_hlc: Some(earlier),
        theirs_hlc: Some(later),
        ours_blob_oid: Some(ours_oid.into()),
        theirs_blob_oid: Some(theirs_oid.into()),
    };
    let (winner, loser) = resolve_add_add_collision(&conflict).unwrap();
    // Theirs has the later HLC, so theirs wins and ours is the losing side.
    assert!(winner.content.contains("Theirs body"));
    assert_eq!(
        loser.losing_blob_oid, ours_oid,
        "losing_blob_oid must carry the losing (ours) side's blob OID, not the winner's"
    );
}

#[test]
fn add_add_loser_blob_oid_is_theirs_when_ours_wins() {
    let later = crate::hlc::Hlc {
        wall_ms: 2000,
        counter: 0,
        node: "nodeA".into(),
    };
    let earlier = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "nodeB".into(),
    };
    let ours_oid = "oid-ours-b";
    let theirs_oid = "oid-theirs-b";
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n".into(),
        ours_hlc: Some(later),
        theirs_hlc: Some(earlier),
        ours_blob_oid: Some(ours_oid.into()),
        theirs_blob_oid: Some(theirs_oid.into()),
    };
    let (winner, loser) = resolve_add_add_collision(&conflict).unwrap();
    // Ours has the later HLC, so ours wins and theirs is the losing side.
    assert!(winner.content.contains("Ours body"));
    assert_eq!(
        loser.losing_blob_oid, theirs_oid,
        "losing_blob_oid must carry the losing (theirs) side's blob OID, not the winner's"
    );
}

#[test]
fn add_add_errors_when_losing_ours_blob_oid_missing() {
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
    let theirs_oid = "oid-theirs-c";
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n".into(),
        ours_hlc: Some(earlier),
        theirs_hlc: Some(later),
        ours_blob_oid: None,
        theirs_blob_oid: Some(theirs_oid.into()),
    };
    // Theirs wins (later HLC); the losing side (ours) has no blob OID recorded.
    let err = resolve_add_add_collision(&conflict)
        .expect_err("a missing losing-side blob OID must be rejected, not silently defaulted");
    assert!(
        err.to_string().contains(conflict.path.as_str()),
        "the error must name the conflict path so an operator can tell which \
         document failed, got: {err}"
    );
}

#[test]
fn add_add_errors_when_losing_theirs_blob_oid_missing() {
    let later = crate::hlc::Hlc {
        wall_ms: 2000,
        counter: 0,
        node: "nodeA".into(),
    };
    let earlier = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "nodeB".into(),
    };
    let ours_oid = "oid-ours-d";
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n".into(),
        ours_hlc: Some(later),
        theirs_hlc: Some(earlier),
        ours_blob_oid: Some(ours_oid.into()),
        theirs_blob_oid: None,
    };
    // Ours wins (later HLC); the losing side (theirs) has no blob OID recorded.
    let err = resolve_add_add_collision(&conflict)
        .expect_err("a missing losing-side blob OID must be rejected, not silently defaulted");
    assert!(
        err.to_string().contains(conflict.path.as_str()),
        "the error must name the conflict path so an operator can tell which \
         document failed, got: {err}"
    );
}

#[test]
fn add_add_succeeds_when_only_the_winning_sides_blob_oid_is_missing() {
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
    let ours_oid = "oid-ours-e";
    let conflict = ConflictFile {
        path: "ddb/20260101120000.md".into(),
        ancestor: None,
        ours: "---\nid: 20260101120000\ntitle: Ours\n---\nOurs body\n".into(),
        theirs: "---\nid: 20260101120000\ntitle: Theirs\n---\nTheirs body\n".into(),
        ours_hlc: Some(earlier),
        theirs_hlc: Some(later),
        ours_blob_oid: Some(ours_oid.into()),
        theirs_blob_oid: None,
    };
    // Theirs wins (later HLC), but it is the WINNER's own blob OID that is
    // missing; the losing (ours) side has one, so this must still succeed.
    let (winner, loser) = resolve_add_add_collision(&conflict).expect(
        "only the losing side's blob OID feeds the derivation; a missing winning-side OID must not error",
    );
    assert!(winner.content.contains("Theirs body"));
    assert_eq!(
        loser.losing_blob_oid, ours_oid,
        "losing_blob_oid must carry the losing (ours) side's blob OID"
    );
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

#[test]
fn add_add_collision_resolves_in_single_commit() {
    // Same same-ID collision setup as `add_add_full_sync_both_survive`, but this
    // test asserts on the git commit shape of the resolution rather than file
    // survival: winner + reassigned loser must land in exactly one two-parent
    // merge commit built directly on B's pre-sync HEAD (no intermediate
    // "reassign" commit between "before sync" and the final merge commit).
    let bare_dir = tempfile::TempDir::new().unwrap();
    git2::Repository::init_bare(bare_dir.path()).unwrap();

    let (_dir_a, repo_a) = temp_repo();
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

    // A pushes, then capture B's HEAD immediately before B syncs.
    repo_a.push("origin", "master").unwrap();
    let pre_sync_head = repo_b.head_oid().unwrap();

    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert_eq!(report.collisions_reassigned, 1);

    // `sync()` always appends one more single-parent "update sync state" commit
    // (via `finalize_sync` -> `update_sync_state`) after resolution, regardless
    // of how resolution itself was committed - so the commit under test is
    // HEAD's parent, not HEAD itself.
    let new_head = repo_b.head_oid().unwrap();
    let head_commit = repo_b
        .repo
        .find_commit(git2::Oid::from_str(&new_head.0).unwrap())
        .unwrap();
    let merge_commit = head_commit.parent(0).unwrap();

    assert_eq!(
        merge_commit.parent_count(),
        2,
        "collision resolution must land in a single two-parent merge commit"
    );

    let pre_sync_oid = git2::Oid::from_str(&pre_sync_head.0).unwrap();
    let parent_oids: Vec<git2::Oid> = (0..merge_commit.parent_count())
        .map(|i| merge_commit.parent_id(i).unwrap())
        .collect();
    assert!(
        parent_oids.contains(&pre_sync_oid),
        "one parent of the merge commit must be exactly B's pre-sync HEAD, proving \
         no intermediate commit was created between 'before sync' and the final merge commit"
    );
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
fn binary_higher_hlc_wins_ours() {
    let (_dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let path = "reference/test/photo.bin";
    // Distinct fake 40-hex blob OIDs; compared as plain strings ("ffff" > "1111").
    let oid_low = "1111111111111111111111111111111111111111";
    let oid_high = "ffffffffffffffffffffffffffffffffffffffff";

    // Ours carries the higher HLC but the LOWER OID: the HLC must dominate.
    let ours_hlc = crate::hlc::Hlc {
        wall_ms: 9000,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let theirs_hlc = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    let conflict = ConflictFile {
        path: path.into(),
        ancestor: None,
        ours: String::new(),
        theirs: String::new(),
        ours_hlc: Some(ours_hlc),
        theirs_hlc: Some(theirs_hlc),
        ours_blob_oid: Some(oid_low.into()),
        theirs_blob_oid: Some(oid_high.into()),
    };

    let resolved = mgr.resolve_binary_ref_conflicts(&[conflict]).unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].0, path);
    assert_eq!(
        resolved[0].1, oid_low,
        "higher HLC (ours) wins even though its blob OID is lower"
    );
}

#[test]
fn binary_higher_hlc_wins_theirs() {
    let (_dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let path = "reference/test/photo.bin";
    let oid_low = "1111111111111111111111111111111111111111";
    let oid_high = "ffffffffffffffffffffffffffffffffffffffff";

    // Theirs carries the higher HLC but the LOWER OID: the HLC must dominate.
    let ours_hlc = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 0,
        node: "aaaaaaaa".into(),
    };
    let theirs_hlc = crate::hlc::Hlc {
        wall_ms: 9000,
        counter: 0,
        node: "bbbbbbbb".into(),
    };
    let conflict = ConflictFile {
        path: path.into(),
        ancestor: None,
        ours: String::new(),
        theirs: String::new(),
        ours_hlc: Some(ours_hlc),
        theirs_hlc: Some(theirs_hlc),
        ours_blob_oid: Some(oid_high.into()),
        theirs_blob_oid: Some(oid_low.into()),
    };

    let resolved = mgr.resolve_binary_ref_conflicts(&[conflict]).unwrap();
    assert_eq!(
        resolved[0].1, oid_low,
        "higher HLC (theirs) wins even though its blob OID is lower"
    );
}

#[test]
fn binary_tie_hlc_picks_higher_blob_oid() {
    let (_dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let path = "reference/test/photo.bin";
    let oid_low = "1111111111111111111111111111111111111111";
    let oid_high = "ffffffffffffffffffffffffffffffffffffffff";

    // Exactly-equal HLCs → the higher blob OID wins.
    let hlc = crate::hlc::Hlc {
        wall_ms: 5000,
        counter: 0,
        node: "samenode".into(),
    };
    let conflict = ConflictFile {
        path: path.into(),
        ancestor: None,
        ours: String::new(),
        theirs: String::new(),
        ours_hlc: Some(hlc.clone()),
        theirs_hlc: Some(hlc.clone()),
        ours_blob_oid: Some(oid_low.into()),
        theirs_blob_oid: Some(oid_high.into()),
    };
    let resolved = mgr.resolve_binary_ref_conflicts(&[conflict]).unwrap();
    assert_eq!(resolved[0].1, oid_high, "higher blob OID wins on HLC tie");

    // Role-swap convergence: swap the OIDs → same winning OID.
    let swapped = ConflictFile {
        path: path.into(),
        ancestor: None,
        ours: String::new(),
        theirs: String::new(),
        ours_hlc: Some(hlc.clone()),
        theirs_hlc: Some(hlc),
        ours_blob_oid: Some(oid_high.into()),
        theirs_blob_oid: Some(oid_low.into()),
    };
    let resolved_swapped = mgr.resolve_binary_ref_conflicts(&[swapped]).unwrap();
    assert_eq!(
        resolved_swapped[0].1, oid_high,
        "role swap converges on the same winning OID"
    );
}

#[test]
fn binary_missing_hlc_picks_higher_blob_oid() {
    let (_dir, repo) = temp_repo();
    register_node(&repo, "Test").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let path = "reference/test/photo.bin";
    let oid_low = "1111111111111111111111111111111111111111";
    let oid_high = "ffffffffffffffffffffffffffffffffffffffff";

    // Both HLCs absent → the higher blob OID wins.
    let conflict = ConflictFile {
        path: path.into(),
        ancestor: None,
        ours: String::new(),
        theirs: String::new(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: Some(oid_low.into()),
        theirs_blob_oid: Some(oid_high.into()),
    };
    let resolved = mgr.resolve_binary_ref_conflicts(&[conflict]).unwrap();
    assert_eq!(
        resolved[0].1, oid_high,
        "higher blob OID wins when both HLCs are missing"
    );

    // Role-swap convergence: swap the OIDs → same winning OID.
    let swapped = ConflictFile {
        path: path.into(),
        ancestor: None,
        ours: String::new(),
        theirs: String::new(),
        ours_hlc: None,
        theirs_hlc: None,
        ours_blob_oid: Some(oid_high.into()),
        theirs_blob_oid: Some(oid_low.into()),
    };
    let resolved_swapped = mgr.resolve_binary_ref_conflicts(&[swapped]).unwrap();
    assert_eq!(
        resolved_swapped[0].1, oid_high,
        "role swap converges on the same winning OID"
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

    // Seed A's machine-local HLC clock far into the future BEFORE A commits, so
    // the write chokepoint auto-stamps a far-future HLC trailer and A's side wins
    // the LWW tie-break — without hand-injecting the trailer into the commit
    // message (the auto-stamp would now double that and shadow the injected value).
    // Format: `{wall_ms}-{counter:04}-{node}` (matches Hlc::Display / Hlc::parse).
    std::fs::write(
        repo_a.path.join(".git/ddb-hlc"),
        "9999999999999-0000-seednode0",
    )
    .unwrap();

    // A commits the non-UTF8 content (the far-future-seeded, winning side).
    repo_a
        .commit_binary_file(bin_path, &non_utf8, "A adds non-utf8 binary")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B commits different content at ordinary wall-clock time (the losing side).
    repo_b
        .commit_binary_file(bin_path, b"placeholder", "B adds placeholder")
        .unwrap();

    // B syncs — A (far-future HLC) wins, content must be byte-exact
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

/// Theirs deletes, ours is untouched → NOT a conflict → file absent, resurrected == 0.
#[test]
fn theirs_deleted_ours_untouched_stays_deleted() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    // A creates a text doogat and pushes
    let id = "20260101120000";
    repo_a
        .commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\nid: {id}\ntitle: Shared\n---\nbody\n"),
            "A creates",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B picks it up
    repo_b.fetch("origin", "master").unwrap();
    repo_b.merge_remote("origin", "master").unwrap();

    // A deletes it and pushes
    repo_a.delete_file(&format!("ddb/{id}.md"), "A deletes").unwrap();
    repo_a.push("origin", "master").unwrap();

    // B does NOTHING to the file.

    // B syncs
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    // The file is gone
    assert!(
        !dir_b.path().join(format!("ddb/{id}.md")).exists(),
        "theirs' deletion must survive when ours is untouched"
    );
    assert_eq!(
        report.resurrected, 0,
        "non-conflicting deletion must not resurrect"
    );
}

/// Theirs deletes, ours edits → delete-vs-edit CONFLICT → edit wins, resurrected == 1.
#[test]
fn theirs_deleted_ours_edited_resurrects_with_marker() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    // A creates the shared doogat and pushes
    let id = "20260101130000";
    repo_a
        .commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\nid: {id}\ntitle: Shared\n---\nbody\n"),
            "A creates",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B picks it up
    repo_b.fetch("origin", "master").unwrap();
    repo_b.merge_remote("origin", "master").unwrap();

    // A deletes it and pushes
    repo_a.delete_file(&format!("ddb/{id}.md"), "A deletes").unwrap();
    repo_a.push("origin", "master").unwrap();

    // B edits it
    repo_b
        .commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\nid: {id}\ntitle: Shared\n---\nB edited body\n"),
            "B edits",
        )
        .unwrap();

    // B syncs
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    // Delete-vs-edit is a conflict
    assert!(
        report.conflicts_resolved > 0,
        "delete-vs-edit is a conflict"
    );
    // The file survived
    let path = dir_b.path().join(format!("ddb/{id}.md"));
    assert!(path.exists(), "edit wins over delete");
    // The resurrected marker is in the content
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.contains("resurrected: true"),
        "resurrected doogat carries the frontmatter marker"
    );
    assert_eq!(
        report.resurrected, 1,
        "delete-vs-edit counts as one resurrection"
    );
}

/// PRD 00166 Success Metric 5: a device skewed ahead does not win indefinitely.
/// Once its HLC is absorbed by the peer during merge, the peer's subsequent writes
/// tie or exceed that peer's former lead, so the skewed lead is not permanent.
/// This exercises the full SyncManager::sync path (not the low-level merge primitive).
#[test]
fn skewed_peer_does_not_win_indefinitely() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_binary_sync_pair();

    let far_future: u64 = u64::MAX / 2;
    let bin_path = "reference/test/asset.bin";

    // Seed A's clock far into the future BEFORE A commits, so the write
    // chokepoint auto-stamps a far-future HLC trailer.
    std::fs::write(
        repo_a.path.join(".git/ddb-hlc"),
        format!("{far_future}-0000-skewnodeA"),
    )
    .unwrap();

    // A commits and pushes (far-future HLC, winning side in round 1).
    repo_a
        .commit_binary_file(bin_path, b"A_CONTENT", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B commits a different edit at ordinary wall-clock time (lower HLC).
    repo_b
        .commit_binary_file(bin_path, b"B_CONTENT", "B edits")
        .unwrap();

    // B syncs — A (far-future HLC) wins round 1.
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = crate::indexer::Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert!(
        report.conflicts_resolved > 0,
        "should have resolved a conflict"
    );

    // ROUND-1 assertion: the skewed peer (A) wins.
    let round1 = std::fs::read(dir_b.path().join(bin_path)).unwrap();
    assert_eq!(
        round1, b"A_CONTENT",
        "round 1: skewed peer (A) wins with far-future HLC"
    );

    // ABSORPTION assertion: B absorbed A's far-future clock during the merge.
    let raw = std::fs::read_to_string(repo_b.repo.path().join("ddb-hlc")).unwrap();
    let b_hlc = crate::hlc::Hlc::parse(raw.trim()).unwrap();
    assert!(
        b_hlc.wall_ms >= far_future,
        "B must absorb A's far-future clock, not stay behind: {} < {far_future}",
        b_hlc.wall_ms
    );

    // CATCH-UP assertion: B's next write now carries HLC >= A's former lead.
    repo_b
        .commit_binary_file(bin_path, b"B_ROUND2", "B round 2")
        .unwrap();
    let head = repo_b.head_oid().unwrap();
    let head_oid = git2::Oid::from_str(&head.0).unwrap();
    let head_commit = repo_b.repo.find_commit(head_oid).unwrap();
    let next = repo_b
        .find_hlc_for_path(&head_commit, bin_path)
        .expect("B's commit must carry an HLC trailer");
    assert!(
        next.wall_ms >= far_future,
        "B's next write must meet/exceed the once-dominant peer's HLC: {} < {far_future}",
        next.wall_ms
    );
}

// --- Add-add collision backlink rewriting ---------------------------------
//
// A collision reassigns the LOSER to a new, content-derived ID. Doogats that
// linked to the loser must follow it; doogats that linked to the WINNER keep
// the contested ID. Both sides can hold such a link, and either side can win,
// so which backlinks get rewritten depends on which side WON - never on which
// side happened to be "theirs".

/// The 14-digit ID both sides mint independently, and that every linker below
/// points at.
const CONTESTED_ID: &str = "20260101120000";

fn make_hlc(wall_ms: u64, node: &str) -> crate::hlc::Hlc {
    crate::hlc::Hlc {
        wall_ms,
        counter: 0,
        node: node.into(),
    }
}

/// A doogat minted at `CONTESTED_ID` by one side of the collision.
fn make_contested_doc(title: &str, body: &str) -> String {
    format!("---\nid: {CONTESTED_ID}\ntitle: {title}\n---\n{body}\n")
}

/// A doogat that links to `CONTESTED_ID`. Returns `(path, content)`.
fn make_linker(id: &str, title: &str) -> (String, String) {
    (
        format!("ddb/{id}.md"),
        format!("---\nid: {id}\ntitle: {title}\n---\nSee [[{CONTESTED_ID}]] for details.\n"),
    )
}

/// What one node ended up with after resolving and committing the collision.
struct BacklinkOutcome {
    /// Content the resolver picked as the winner, before the merge commit.
    winner_content: String,
    /// Content read back from `ddb/{CONTESTED_ID}.md` AFTER the merge commit -
    /// the committed tree, not the resolver's return value.
    committed_winner: String,
    /// Content the resolver set aside as the loser, before the merge commit.
    loser_content: String,
    /// Content read back from the loser's new path AFTER the merge commit.
    committed_loser: String,
    /// The reassigned, content-derived ID the loser now lives under.
    loser_new_id: String,
    /// Post-merge content of each linker that existed only on this node's side,
    /// in the order the caller supplied them.
    ours_linkers: Vec<String>,
    /// Post-merge content of each linker that arrived only from the peer, in the
    /// order the caller supplied them.
    theirs_linkers: Vec<String>,
}

/// Drive one add-add collision to a committed merge on a node whose own doogat
/// is `ours_doc` and whose peer contributes `theirs_doc`, both at
/// `ddb/{CONTESTED_ID}.md`. Each side also owns one or more doogats linking to
/// `CONTESTED_ID` (`(path, content)` pairs), so the caller can assert which
/// backlinks the merge rewrote. IDs, titles and bodies are all caller-supplied:
/// nothing about which side wins, or which files must move, is knowable from a
/// literal baked into the fixture.
///
/// Winner selection is the real `resolve_add_add_collision`, driven by the two
/// HLCs, so callers control - and must verify - which side wins.
fn resolve_collision_with_backlinks(
    ours_doc: &str,
    ours_linkers: &[(String, String)],
    ours_hlc: crate::hlc::Hlc,
    theirs_doc: &str,
    theirs_linkers: &[(String, String)],
    theirs_hlc: crate::hlc::Hlc,
) -> BacklinkOutcome {
    let bare_dir = tempfile::TempDir::new().unwrap();
    git2::Repository::init_bare(bare_dir.path()).unwrap();

    // Peer repo ("theirs"), then a clone that plays the node under test ("ours").
    let (_dir_peer, repo_peer) = temp_repo();
    repo_peer
        .add_remote("origin", bare_dir.path().to_str().unwrap())
        .unwrap();
    repo_peer.push("origin", "master").unwrap();

    let dir_ours = tempfile::TempDir::new().unwrap();
    git2::Repository::clone(bare_dir.path().to_str().unwrap(), dir_ours.path()).unwrap();
    let repo_ours = GitRepo::open(dir_ours.path()).unwrap();

    let contested_path = format!("ddb/{CONTESTED_ID}.md");

    // Peer creates its doogat at the contested ID plus its doogats linking to it.
    for (path, content) in theirs_linkers {
        repo_peer
            .commit_file(path, content, "peer adds linker")
            .unwrap();
    }
    repo_peer
        .commit_file(&contested_path, theirs_doc, "peer creates contested")
        .unwrap();
    repo_peer.push("origin", "master").unwrap();

    // The node under test independently mints the same ID, with its own linkers.
    for (path, content) in ours_linkers {
        repo_ours
            .commit_file(path, content, "node adds linker")
            .unwrap();
    }
    repo_ours
        .commit_file(&contested_path, ours_doc, "node creates contested")
        .unwrap();

    repo_ours.fetch("origin", "master").unwrap();
    let theirs_oid = match repo_ours.merge_remote("origin", "master").unwrap() {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(
                conflicts.len(),
                1,
                "only the same-ID doogat may conflict; the linkers live at distinct paths: {conflicts:?}"
            );
            assert_eq!(conflicts[0].path, contested_path);
            theirs_oid
        }
        other => panic!("expected an add-add conflict, got {other:?}"),
    };

    let conflict = ConflictFile {
        path: contested_path.clone(),
        ancestor: None,
        ours: ours_doc.to_string(),
        theirs: theirs_doc.to_string(),
        ours_hlc: Some(ours_hlc),
        theirs_hlc: Some(theirs_hlc),
        ours_blob_oid: Some(repo_ours.repo.blob(ours_doc.as_bytes()).unwrap().to_string()),
        theirs_blob_oid: Some(
            repo_ours
                .repo
                .blob(theirs_doc.as_bytes())
                .unwrap()
                .to_string(),
        ),
    };
    let (winner, loser) = resolve_add_add_collision(&conflict).unwrap();

    let new_id =
        crate::id_minting::derive_content_id(&loser.old_id, &loser.losing_blob_oid, |_| false);
    let loser_path =
        crate::git_ops::doogat_path(&new_id.0, loser.type_name.as_deref(), loser.folder);
    let winner_content = winner.content.clone();
    let loser_content = loser.content.clone();

    repo_ours
        .commit_merge(
            &[(winner.path.as_str(), winner.content.as_str())],
            &[],
            &[loser],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let committed_loser = repo_ours
        .read_file(&loser_path)
        .expect("the loser must be reassigned to its content-derived path");

    // Every read below comes from HEAD after the merge commit, i.e. the
    // committed tree - never from the resolver's in-memory return value.
    let read_back = |linkers: &[(String, String)]| -> Vec<String> {
        linkers
            .iter()
            .map(|(path, _)| repo_ours.read_file(path).unwrap())
            .collect()
    };
    BacklinkOutcome {
        winner_content,
        committed_winner: repo_ours
            .read_file(&contested_path)
            .expect("the winner must still occupy the contested path after the merge"),
        loser_content,
        committed_loser,
        loser_new_id: new_id.0,
        ours_linkers: read_back(ours_linkers),
        theirs_linkers: read_back(theirs_linkers),
    }
}

/// The contested path must still hold the winner after the merge: the unchanged
/// contested ID and the winner's body, byte-for-byte what the resolver picked.
/// Doubles as the setup check - a test states the body it expects to win, so a
/// setup that silently produced the other winner fails here instead of quietly
/// asserting the wrong direction.
fn assert_winner_kept_contested_id(outcome: &BacklinkOutcome, winner_body: &str) {
    assert!(
        outcome
            .committed_winner
            .contains(&format!("id: {CONTESTED_ID}")),
        "the winner must still carry the contested ID at ddb/{CONTESTED_ID}.md, got {:?}",
        outcome.committed_winner
    );
    assert!(
        outcome.committed_winner.contains(winner_body),
        "test setup invalid: the higher HLC must make the expected side win, so \
         ddb/{CONTESTED_ID}.md should hold {winner_body:?}, got {:?}",
        outcome.committed_winner
    );
    assert_eq!(
        outcome.committed_winner, outcome.winner_content,
        "the committed tree must hold exactly the doogat the resolver picked; the \
         loser reassignment must not touch the winner's own file"
    );
}

/// The mirror of `assert_winner_kept_contested_id` for the losing side: the
/// reassignment only removes the duplicate ID if the loser's COMMITTED file
/// actually declares the new ID and has stopped claiming the contested one -
/// otherwise two doogats still answer to `CONTESTED_ID`, which is the exact
/// state this whole reassignment exists to prevent. Nothing but the ID field may
/// change, so the loser's own body survives the move.
fn assert_loser_moved_to_its_new_id(outcome: &BacklinkOutcome) {
    let new_id = &outcome.loser_new_id;
    assert!(
        outcome.committed_loser.contains(&format!("id: {new_id}")),
        "the reassigned loser must declare its new ID {new_id}, got {:?}",
        outcome.committed_loser
    );
    assert!(
        !outcome
            .committed_loser
            .contains(&format!("id: {CONTESTED_ID}")),
        "the loser must stop claiming the contested ID {CONTESTED_ID} - leaving it there \
         means two doogats share one ID, got {:?}",
        outcome.committed_loser
    );
    assert_eq!(
        outcome.committed_loser,
        crate::parser::rewrite_id_field(&outcome.loser_content, new_id).unwrap(),
        "the reassignment must rewrite the loser's ID field and change nothing else"
    );
}

/// Backlinks on the WINNING side already point at the ID the winner keeps, so
/// every one of them must survive the merge byte-for-byte.
fn assert_backlinks_untouched(
    after: &[String],
    before: &[(String, String)],
    winning_side: &str,
    loser_new_id: &str,
) {
    assert_eq!(after.len(), before.len(), "one result per linker");
    for (got, (path, want)) in after.iter().zip(before) {
        assert_eq!(
            got, want,
            "{winning_side} won, so {path} points at the WINNER's unchanged ID and must \
             survive untouched; the merge repointed it (the loser's new ID is {loser_new_id})"
        );
    }
}

/// Backlinks on the LOSING side were written against the doogat that lost, so
/// every one of them must follow it to its new ID - and nothing else in those
/// files may change, which also rules out a rewrite that overwrites the wrong
/// file or writes the same content everywhere.
fn assert_backlinks_follow_loser(
    after: &[String],
    before: &[(String, String)],
    losing_side: &str,
    loser_new_id: &str,
) {
    assert_eq!(after.len(), before.len(), "one result per linker");
    for (got, (path, before_content)) in after.iter().zip(before) {
        let expected = before_content.replace(
            &format!("[[{CONTESTED_ID}]]"),
            &format!("[[{loser_new_id}]]"),
        );
        assert_eq!(
            got, &expected,
            "{losing_side} lost, so {path} must be repointed at the loser's new ID \
             {loser_new_id} with the rest of the file untouched"
        );
    }
}

/// OURS wins: the winner keeps the contested ID, so EVERY ours-side backlink
/// already points at the surviving doogat and must come out of the merge
/// byte-for-byte unchanged. Fails if the loser-reassignment rewrite is scoped to
/// "every file that did not come from theirs" instead of "every file that did
/// not come from the WINNING side".
#[test]
fn ours_wins_keeps_every_ours_side_backlink_on_the_unchanged_winner_id() {
    let ours_doc = make_contested_doc("Kite Draft", "kite body");
    let theirs_doc = make_contested_doc("Heron Draft", "heron body");
    let ours_linkers = vec![
        make_linker("20260404110000", "Kite Index"),
        make_linker("20260505120000", "Kite Journal"),
    ];
    let theirs_linkers = vec![
        make_linker("20260606130000", "Heron Index"),
        make_linker("20260707140000", "Heron Journal"),
    ];

    let outcome = resolve_collision_with_backlinks(
        &ours_doc,
        &ours_linkers,
        make_hlc(2000, "nodeKite"),
        &theirs_doc,
        &theirs_linkers,
        make_hlc(1000, "nodeHeron"),
    );

    assert_winner_kept_contested_id(&outcome, "kite body");
    assert_loser_moved_to_its_new_id(&outcome);
    assert_backlinks_untouched(
        &outcome.ours_linkers,
        &ours_linkers,
        "ours",
        &outcome.loser_new_id,
    );
}

/// OURS wins: the peer's backlinks were written against the doogat that lost, so
/// EVERY one of them must follow that doogat to its new content-derived ID.
/// Leaving any on `CONTESTED_ID` would silently re-aim it at a different doogat
/// (the winner); rewriting only one of them strands the rest.
#[test]
fn ours_wins_rewrites_every_theirs_side_backlink_to_the_losers_new_id() {
    let ours_doc = make_contested_doc("Willow Draft", "willow body");
    let theirs_doc = make_contested_doc("Cedar Draft", "cedar body");
    let ours_linkers = vec![
        make_linker("20261111150000", "Willow Ledger"),
        make_linker("20261212160000", "Willow Digest"),
    ];
    let theirs_linkers = vec![
        make_linker("20260808170000", "Cedar Ledger"),
        make_linker("20260909180000", "Cedar Digest"),
    ];

    let outcome = resolve_collision_with_backlinks(
        &ours_doc,
        &ours_linkers,
        make_hlc(2000, "nodeWillow"),
        &theirs_doc,
        &theirs_linkers,
        make_hlc(1000, "nodeCedar"),
    );

    assert_winner_kept_contested_id(&outcome, "willow body");
    assert_loser_moved_to_its_new_id(&outcome);
    assert_backlinks_follow_loser(
        &outcome.theirs_linkers,
        &theirs_linkers,
        "theirs",
        &outcome.loser_new_id,
    );
}

/// THEIRS wins: the mirror image, pinned so a winner-scoped rewrite cannot
/// silently invert it - every peer backlink points at the winner and stays put,
/// while every ours-side backlink follows the loser to its new ID.
///
/// Two of the linkers here deliberately contradict their own side's wording, so
/// no rewrite decision can be read off what a file happens to say: a losing-side
/// linker shares no word with either contested doogat (yet must be rewritten),
/// and a winning-side linker is named after the LOSER (yet must be left alone).
/// Which files move is decided by which side WON, not by which doogat they read
/// like.
#[test]
fn theirs_wins_keeps_theirs_side_backlinks_and_rewrites_ours_side_backlinks() {
    let ours_doc = make_contested_doc("Otter Draft", "otter body");
    let theirs_doc = make_contested_doc("Badger Draft", "badger body");
    let ours_linkers = vec![
        make_linker("20250101010000", "Otter Atlas"),
        make_linker("20250202020000", "Otter Ledger"),
        make_linker("20250505050000", "Quarterly Roundup"),
    ];
    let theirs_linkers = vec![
        make_linker("20250303030000", "Badger Atlas"),
        make_linker("20250404040000", "Badger Ledger"),
        make_linker("20250606060000", "Otter Retrospective"),
    ];

    let outcome = resolve_collision_with_backlinks(
        &ours_doc,
        &ours_linkers,
        make_hlc(1000, "nodeOtter"),
        &theirs_doc,
        &theirs_linkers,
        make_hlc(2000, "nodeBadger"),
    );

    assert_winner_kept_contested_id(&outcome, "badger body");
    assert_loser_moved_to_its_new_id(&outcome);
    assert_backlinks_untouched(
        &outcome.theirs_linkers,
        &theirs_linkers,
        "theirs",
        &outcome.loser_new_id,
    );
    assert_backlinks_follow_loser(
        &outcome.ours_linkers,
        &ours_linkers,
        "ours",
        &outcome.loser_new_id,
    );
}

/// Two nodes resolve the SAME collision with the ours/theirs roles reversed
/// (node Bravo sees Alpha's doogat arrive; node Alpha sees Bravo's). Every
/// doogat and linker keeps its identity across the swap, so both nodes must end
/// up with the same link target in each linker - otherwise the two replicas
/// disagree about which doogat a link points at.
#[test]
fn both_nodes_converge_on_the_same_backlink_targets_after_role_swap() {
    let doc_alpha = make_contested_doc("Alpha Draft", "alpha body");
    let doc_bravo = make_contested_doc("Bravo Draft", "bravo body");
    let linkers_alpha = vec![
        make_linker("20240505050000", "Alpha Roster"),
        make_linker("20240606060000", "Alpha Digest"),
    ];
    let linkers_bravo = vec![
        make_linker("20240707070000", "Bravo Roster"),
        make_linker("20240808080000", "Bravo Digest"),
    ];

    // On node Bravo: Bravo's doogat is "ours", Alpha's arrives as "theirs".
    let on_bravo = resolve_collision_with_backlinks(
        &doc_bravo,
        &linkers_bravo,
        make_hlc(1000, "nodeBravo"),
        &doc_alpha,
        &linkers_alpha,
        make_hlc(2000, "nodeAlpha"),
    );

    // On node Alpha: the same collision, roles (and their HLCs) reversed.
    let on_alpha = resolve_collision_with_backlinks(
        &doc_alpha,
        &linkers_alpha,
        make_hlc(2000, "nodeAlpha"),
        &doc_bravo,
        &linkers_bravo,
        make_hlc(1000, "nodeBravo"),
    );

    // Alpha's HLC is the higher one on both nodes, so Alpha's doogat keeps the
    // contested ID whichever side it arrives from.
    assert_winner_kept_contested_id(&on_bravo, "alpha body");
    assert_winner_kept_contested_id(&on_alpha, "alpha body");
    assert_loser_moved_to_its_new_id(&on_bravo);
    assert_loser_moved_to_its_new_id(&on_alpha);
    assert_eq!(
        on_bravo.committed_winner, on_alpha.committed_winner,
        "both nodes must keep the same doogat at the contested ID"
    );
    assert_eq!(
        on_bravo.loser_new_id, on_alpha.loser_new_id,
        "both nodes must mint the same new ID for the losing doogat"
    );
    assert_eq!(
        on_bravo.ours_linkers, on_alpha.theirs_linkers,
        "Bravo's linkers must end up pointing at the same doogat on both nodes"
    );
    assert_eq!(
        on_bravo.theirs_linkers, on_alpha.ours_linkers,
        "Alpha's linkers must end up pointing at the same doogat on both nodes"
    );
}

/// Runs the same add-add collision through the real `SyncManager::sync` path in a
/// freshly built pair of repos, with `a_pushes_first` controlling which node pushes
/// first and therefore which node performs the resolution. Returns the sorted
/// surviving doogat filenames from whichever node resolved the collision.
fn resolve_collision_with_push_order(a_pushes_first: bool) -> Vec<String> {
    let (dir_a, repo_a, dir_b, repo_b, _bare_dir) = setup_binary_sync_pair();

    // Both create the same-ID doogat, same content, same commit order (A then B)
    // in every call, regardless of which node pushes first.
    let id = "20260101120000";
    repo_a
        .commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\nid: {id}\ntitle: From A\n---\nA body\n"),
            "A creates",
        )
        .unwrap();
    // Force B's commit into a strictly later millisecond than A's. Without this,
    // both commits can land in the same wall-clock millisecond, and `lww_pick`
    // then falls through to the HLC counter/node tiebreak. `setup_binary_sync_pair`
    // already puts A's clock ahead of B's inside that shared millisecond (its
    // `Hlc::recv` bumps A's counter past B's during peer registration), so a
    // same-millisecond tie picks A, while a millisecond boundary landing between
    // these two commits picks B - the winner becomes a coin flip on scheduling
    // jitter, and the two universes below can disagree. Sleeping past the
    // millisecond boundary here guarantees B.wall_ms > A.wall_ms every time, so
    // `lww_pick` always takes the wall-clock branch and never reaches the
    // counter/node tiebreak, making the winner deterministic across universes.
    std::thread::sleep(std::time::Duration::from_millis(5));
    repo_b
        .commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\nid: {id}\ntitle: From B\n---\nB body\n"),
            "B creates",
        )
        .unwrap();

    let resolver_dir = if a_pushes_first {
        repo_a.push("origin", "master").unwrap();
        let db_b = dir_b.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
        let index_b = crate::indexer::Index::open(&db_b).unwrap();
        let mut mgr_b = SyncManager::open(&repo_b).unwrap();
        let report = mgr_b.sync("origin", "master", &index_b).unwrap();
        assert_eq!(
            report.collisions_reassigned, 1,
            "A pushes first: B must reassign exactly one collision"
        );
        dir_b.path()
    } else {
        repo_b.push("origin", "master").unwrap();
        let db_a = dir_a.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_a.parent().unwrap()).unwrap();
        let index_a = crate::indexer::Index::open(&db_a).unwrap();
        let mut mgr_a = SyncManager::open(&repo_a).unwrap();
        let report = mgr_a.sync("origin", "master", &index_a).unwrap();
        assert_eq!(
            report.collisions_reassigned, 1,
            "B pushes first: A must reassign exactly one collision"
        );
        dir_a.path()
    };

    let mut files: Vec<String> = std::fs::read_dir(resolver_dir.join("ddb"))
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
    files.sort();

    assert_eq!(
        files.len(),
        2,
        "both doogats should survive (a_pushes_first={a_pushes_first}): {files:?}"
    );

    files
}

/// Resolving the SAME add-add collision in two independent universes that differ
/// ONLY in which node pushes first (and therefore which node performs the
/// resolution) must land on the same winner and the same reassigned loser ID. A
/// wall-clock-minted loser ID would fail this (the two universes resolve at
/// different instants), and a role-dependent winner rule like "theirs always
/// wins" would also fail it, since the roles are swapped between universes.
#[test]
fn add_add_collision_resolution_is_independent_of_push_direction() {
    let id = "20260101120000";

    // Universe 1: A pushes first, B fetches and resolves.
    let universe_1 = resolve_collision_with_push_order(true);
    // Universe 2: freshly built repos, byte-identical content, same commit order -
    // but B pushes first and A resolves.
    let universe_2 = resolve_collision_with_push_order(false);

    assert_eq!(
        universe_1, universe_2,
        "resolution must be independent of which node pushes first: universe 1 \
         (A pushes first) produced {universe_1:?}, universe 2 (B pushes first) \
         produced {universe_2:?}"
    );

    let winner_name = format!("{id}.md");
    assert!(
        universe_1.contains(&winner_name),
        "the winner must keep the originally-colliding id: {universe_1:?}"
    );
    assert!(
        universe_1.iter().any(|f| f != &winner_name),
        "a real reassignment must have happened: the other filename must not equal \
         the colliding id: {universe_1:?}"
    );
}

