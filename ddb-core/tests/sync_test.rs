use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use ddb_core::sync_manager::{self, SyncManager};

fn setup_two_nodes() -> (
    tempfile::TempDir,
    GitRepo, // Node A
    tempfile::TempDir,
    GitRepo,           // Node B
    tempfile::TempDir, // Bare remote
) {
    // Bare remote
    let bare_dir = tempfile::TempDir::new().unwrap();
    git2::Repository::init_bare(bare_dir.path()).unwrap();

    // Node A
    let dir_a = tempfile::TempDir::new().unwrap();
    let repo_a = GitRepo::init(dir_a.path()).unwrap();
    repo_a
        .add_remote("origin", bare_dir.path().to_str().unwrap())
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    sync_manager::register_node(&repo_a, "NodeA").unwrap();
    repo_a.push("origin", "master").unwrap();

    // Node B (clone)
    let dir_b = tempfile::TempDir::new().unwrap();
    git2::Repository::clone(bare_dir.path().to_str().unwrap(), dir_b.path()).unwrap();
    let repo_b = GitRepo::open(dir_b.path()).unwrap();
    sync_manager::register_node(&repo_b, "NodeB").unwrap();
    repo_b.push("origin", "master").unwrap();

    // Sync A to get B's node file
    repo_a.fetch("origin", "master").unwrap();
    repo_a.merge_remote("origin", "master").unwrap();

    (dir_a, repo_a, dir_b, repo_b, bare_dir)
}

#[test]
fn two_node_sync_no_conflicts() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_two_nodes();

    // A creates a doogat
    let content = "---\nid: 20260226120000\ntitle: Note From A\ntags:\n  - test\n---\nBody from A.";
    repo_a
        .commit_file("ddb/20260226120000.md", content, "A creates note")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B syncs
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();

    assert_eq!(report.conflicts_resolved, 0);

    // B should see the doogat
    let b_content = repo_b.read_file("ddb/20260226120000.md").unwrap();
    assert!(b_content.contains("Note From A"));

    // Index should have it
    let results = index_b.search("Body").unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn two_node_sync_with_conflict_resolution() {
    let (dir_a, repo_a, dir_b, repo_b, _bare) = setup_two_nodes();

    // Both nodes start with same doogat
    let original = "---\nid: 20260226120000\ntitle: Original\ntags:\n  - shared\n---\nOriginal body.\n---\n- source:: Wikipedia";
    repo_a
        .commit_file("ddb/20260226120000.md", original, "add original")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B gets the original
    let db_b = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_b.parent().unwrap()).unwrap();
    let index_b = Index::open(&db_b).unwrap();
    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    mgr_b.sync("origin", "master", &index_b).unwrap();

    // A pulls B's sync state update, then edits title AND body (same lines)
    repo_a.fetch("origin", "master").unwrap();
    repo_a.merge_remote("origin", "master").unwrap();

    let a_edit = "---\nid: 20260226120000\ntitle: Title From A\ntags:\n  - shared\n---\nBody edited by A.\n---\n- source:: Wikipedia\n- edited-by:: A";
    repo_a
        .commit_file("ddb/20260226120000.md", a_edit, "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B edits: changes same body line AND adds different reference field
    let b_edit = "---\nid: 20260226120000\ntitle: Title From B\ntags:\n  - shared\n---\nBody edited by B.\n---\n- source:: Wikipedia\n- edited-by:: B";
    repo_b
        .commit_file("ddb/20260226120000.md", b_edit, "B edits")
        .unwrap();

    // B syncs — should resolve conflict via CRDT
    let report = mgr_b.sync("origin", "master", &index_b).unwrap();
    assert!(report.conflicts_resolved > 0);

    // The resolved file should parse without error
    let resolved = repo_b.read_file("ddb/20260226120000.md").unwrap();
    let parsed = ddb_core::parser::parse(&resolved, "ddb/20260226120000.md").unwrap();

    // A's title change should be present
    assert!(parsed.meta.title.is_some());

    // B's reference changes should be present
    assert!(resolved.contains("edited-by"));

    // A syncs back — should be clean fast-forward
    let db_a = dir_a.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_a.parent().unwrap()).unwrap();
    let index_a = Index::open(&db_a).unwrap();
    let mut mgr_a = SyncManager::open(&repo_a).unwrap();
    let report_a = mgr_a.sync("origin", "master", &index_a).unwrap();
    assert_eq!(report_a.conflicts_resolved, 0);

    // Both repos should have identical doogat content
    let a_content = repo_a.read_file("ddb/20260226120000.md").unwrap();
    let b_content = repo_b.read_file("ddb/20260226120000.md").unwrap();
    assert_eq!(a_content, b_content);
}
