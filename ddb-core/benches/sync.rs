use std::path::Path;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use tempfile::TempDir;
use ddb_core::compaction;
use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use ddb_core::sync_manager::{register_node, SyncManager};

const DOOGAT_COUNT_1K: usize = 1000;
const DOOGAT_COUNT_5K: usize = 5000;

fn doogat_content(i: usize) -> String {
    format!(
        "---\ntitle: Note {i}\ndate: 2026-01-01\ntags:\n  - bench\n---\nBody of doogat {i}.\n---\n- source:: bench-{i}"
    )
}

fn doogat_path(i: usize) -> String {
    format!("ddb/{:014}.md", 20260101000000u64 + i as u64)
}

/// Set up bare remote + repo A (populated, pushed) + repo B (cloned).
fn setup_sync_pair(
    bare_dir: &Path,
    dir_a: &Path,
    dir_b: &Path,
    count: usize,
) -> (GitRepo, Index, GitRepo, Index) {
    git2::Repository::init_bare(bare_dir).unwrap();

    // Repo A: init, populate, push
    let repo_a = GitRepo::init(dir_a).unwrap();
    let files: Vec<(String, String)> = (0..count)
        .map(|i| (doogat_path(i), doogat_content(i)))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    repo_a.commit_files(&refs, "seed").unwrap();
    repo_a
        .add_remote("origin", bare_dir.to_str().unwrap())
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    register_node(&repo_a, "NodeA").unwrap();

    let db_a = dir_a.join("index.db");
    let index_a = Index::open(&db_a).unwrap();
    index_a.rebuild(&repo_a).unwrap();

    // Repo B: clone
    let _raw = git2::Repository::clone(bare_dir.to_str().unwrap(), dir_b).unwrap();
    let repo_b = GitRepo::open(dir_b).unwrap();
    register_node(&repo_b, "NodeB").unwrap();

    let db_b = dir_b.join("index.db");
    let index_b = Index::open(&db_b).unwrap();
    index_b.rebuild(&repo_b).unwrap();

    (repo_a, index_a, repo_b, index_b)
}

fn bench_sync_fast_forward(c: &mut Criterion) {
    c.bench_function("sync/fast_forward_1k", |b| {
        b.iter_batched(
            || {
                let bare = TempDir::new().unwrap();
                let da = TempDir::new().unwrap();
                let db = TempDir::new().unwrap();
                let (repo_a, _idx_a, repo_b, index_b) =
                    setup_sync_pair(bare.path(), da.path(), db.path(), DOOGAT_COUNT_1K);

                // A adds 10 new doogats and pushes
                let new_files: Vec<(String, String)> = (DOOGAT_COUNT_1K..DOOGAT_COUNT_1K + 10)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = new_files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo_a.commit_files(&refs, "add 10").unwrap();
                repo_a.push("origin", "master").unwrap();

                (bare, da, db, repo_b, index_b)
            },
            |(_bare, _da, _db, repo_b, index_b)| {
                let mut mgr = SyncManager::open(&repo_b).unwrap();
                mgr.sync("origin", "master", &index_b).unwrap();
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_sync_fast_forward_5k(c: &mut Criterion) {
    c.bench_function("sync/fast_forward_5k", |b| {
        b.iter_batched(
            || {
                let bare = TempDir::new().unwrap();
                let da = TempDir::new().unwrap();
                let db = TempDir::new().unwrap();
                let (repo_a, _idx_a, repo_b, index_b) =
                    setup_sync_pair(bare.path(), da.path(), db.path(), DOOGAT_COUNT_5K);

                // A adds 10 new doogats and pushes
                let new_files: Vec<(String, String)> = (DOOGAT_COUNT_5K..DOOGAT_COUNT_5K + 10)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = new_files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo_a.commit_files(&refs, "add 10").unwrap();
                repo_a.push("origin", "master").unwrap();

                (bare, da, db, repo_b, index_b)
            },
            |(_bare, _da, _db, repo_b, index_b)| {
                let mut mgr = SyncManager::open(&repo_b).unwrap();
                mgr.sync("origin", "master", &index_b).unwrap();
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_compact(c: &mut Criterion) {
    c.bench_function("sync/compact", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().unwrap();
                let repo = GitRepo::init(dir.path()).unwrap();
                let files: Vec<(String, String)> = (0..DOOGAT_COUNT_1K)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo.commit_files(&refs, "seed").unwrap();
                register_node(&repo, "BenchNode").unwrap();
                (dir, repo)
            },
            |(_dir, repo)| {
                let mgr = SyncManager::open(&repo).unwrap();
                let opts = ddb_core::types::CompactOptions {
                    force: true,
                    skip_backup: true,
                    ..Default::default()
                };
                compaction::compact(&repo, &mgr, &opts).unwrap();
            },
            BatchSize::PerIteration,
        );
    });
}

criterion_group!(
    benches,
    bench_sync_fast_forward,
    bench_sync_fast_forward_5k,
    bench_compact
);
criterion_main!(benches);
