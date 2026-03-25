use ddb_core::compaction;
use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use ddb_core::sync_manager::{self, SyncManager};
use ddb_core::types::CompactOptions;

fn setup_repo_with_maintenance() -> (tempfile::TempDir, GitRepo) {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    let toml = "[maintenance]\nauto_enabled = true\n";
    repo.commit_file(".ddb.toml", toml, "enable maintenance")
        .unwrap();
    (dir, repo)
}

#[test]
fn compact_with_auto_maintenance_succeeds() {
    let (_dir, repo) = setup_repo_with_maintenance();
    sync_manager::register_node(&repo, "test-node").unwrap();

    let opts = CompactOptions {
        force: true,
        skip_backup: true,
        ..Default::default()
    };
    let mgr = SyncManager::open(&repo).unwrap();
    let report = compaction::compact(&repo, &mgr, &opts).unwrap();
    assert!(report.gc_success);
}

#[test]
fn sync_with_auto_maintenance_succeeds() {
    // Bare remote
    let bare_dir = tempfile::TempDir::new().unwrap();
    git2::Repository::init_bare(bare_dir.path()).unwrap();

    // Node A with maintenance enabled
    let dir_a = tempfile::TempDir::new().unwrap();
    let repo_a = GitRepo::init(dir_a.path()).unwrap();
    let toml = "[maintenance]\nauto_enabled = true\n";
    repo_a
        .commit_file(".ddb.toml", toml, "enable maintenance")
        .unwrap();
    repo_a
        .add_remote("origin", bare_dir.path().to_str().unwrap())
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    sync_manager::register_node(&repo_a, "NodeA").unwrap();
    repo_a.push("origin", "master").unwrap();

    // Node B (clone, register, push)
    let dir_b = tempfile::TempDir::new().unwrap();
    git2::Repository::clone(bare_dir.path().to_str().unwrap(), dir_b.path()).unwrap();
    let repo_b = GitRepo::open(dir_b.path()).unwrap();
    sync_manager::register_node(&repo_b, "NodeB").unwrap();
    repo_b.push("origin", "master").unwrap();

    // A fetches B's node registration
    repo_a.fetch("origin", "master").unwrap();
    repo_a.merge_remote("origin", "master").unwrap();

    // A creates a doogat and pushes
    repo_a
        .commit_file(
            "ddb/20260313000000.md",
            "---\nid: 20260313000000\ntitle: Test\ntags: []\n---\nBody.",
            "create",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // B syncs — auto-maintenance fires after sync
    let db_path = dir_b.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index = Index::open(&db_path).unwrap();
    index.rebuild(&repo_b).unwrap();

    let mut mgr_b = SyncManager::open(&repo_b).unwrap();
    let report = mgr_b.sync("origin", "master", &index).unwrap();
    assert!(report.commits_transferred >= 1);
}

#[test]
fn compact_with_maintenance_disabled_still_succeeds() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    // Default config: auto_enabled = false
    sync_manager::register_node(&repo, "test-node").unwrap();

    let opts = CompactOptions {
        force: true,
        skip_backup: true,
        ..Default::default()
    };
    let mgr = SyncManager::open(&repo).unwrap();
    let report = compaction::compact(&repo, &mgr, &opts).unwrap();
    assert!(report.gc_success);
}

#[test]
fn maintenance_run_explicit_succeeds() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    let report = ddb_core::maintenance::run(&repo.path, None).unwrap();
    assert!(report.success);
}

#[test]
fn maintenance_config_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    // Default: disabled
    let config = repo.load_config().unwrap();
    assert!(!config.maintenance.auto_enabled);

    // Enable
    let toml = "[maintenance]\nauto_enabled = true\n";
    repo.commit_file(".ddb.toml", toml, "enable").unwrap();
    let config = repo.load_config().unwrap();
    assert!(config.maintenance.auto_enabled);

    // Disable
    let toml = "[maintenance]\nauto_enabled = false\n";
    repo.commit_file(".ddb.toml", toml, "disable").unwrap();
    let config = repo.load_config().unwrap();
    assert!(!config.maintenance.auto_enabled);
}
