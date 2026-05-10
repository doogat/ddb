use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};
use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use tempfile::TempDir;

const DOOGAT_COUNT_1K: usize = 1000;
const DOOGAT_COUNT_5K: usize = 5000;

fn doogat_content(i: usize) -> String {
    let word = match i % 5 {
        0 => "architecture",
        1 => "refactoring",
        2 => "deployment",
        3 => "performance",
        _ => "documentation",
    };
    format!(
        "---\ntitle: Note about {word} {i}\ndate: 2026-01-01\ntags:\n  - bench\n  - {word}\n---\n\
         This doogat discusses {word} in the context of item {i}.\n\
         Some additional body text for search indexing.\n\
         ---\n- source:: bench-{i}"
    )
}

fn doogat_path(i: usize) -> String {
    format!("ddb/{:014}.md", 20260101000000u64 + i as u64)
}

fn populated_repo_and_index(repo_dir: &Path, db_path: &Path, count: usize) -> (GitRepo, Index) {
    let repo = GitRepo::init(repo_dir).unwrap();
    let files: Vec<(String, String)> = (0..count)
        .map(|i| (doogat_path(i), doogat_content(i)))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    repo.commit_files(&refs, "seed").unwrap();

    let index = Index::open(db_path).unwrap();
    index.rebuild(&repo).unwrap();
    (repo, index)
}

fn bench_search(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let repo_dir = dir.path().join("repo");
    let (_repo, index) = populated_repo_and_index(&repo_dir, &db_path, DOOGAT_COUNT_1K);

    c.bench_function("search/fts_1k", |b| {
        b.iter(|| {
            index.search("architecture").unwrap();
        });
    });
}

fn bench_query_raw(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let repo_dir = dir.path().join("repo");
    let (_repo, index) = populated_repo_and_index(&repo_dir, &db_path, DOOGAT_COUNT_1K);

    c.bench_function("search/sql_select_1k", |b| {
        b.iter(|| {
            index
                .query_raw(
                    "SELECT id, title FROM doogats WHERE title LIKE '%architecture%' LIMIT 10",
                )
                .unwrap();
        });
    });
}

fn bench_search_5k(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let repo_dir = dir.path().join("repo");
    let (_repo, index) = populated_repo_and_index(&repo_dir, &db_path, DOOGAT_COUNT_5K);

    c.bench_function("search/fts_5k", |b| {
        b.iter(|| {
            index.search("architecture").unwrap();
        });
    });
}

fn bench_query_raw_5k(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let repo_dir = dir.path().join("repo");
    let (_repo, index) = populated_repo_and_index(&repo_dir, &db_path, DOOGAT_COUNT_5K);

    c.bench_function("search/sql_select_5k", |b| {
        b.iter(|| {
            index
                .query_raw(
                    "SELECT id, title FROM doogats WHERE title LIKE '%architecture%' LIMIT 10",
                )
                .unwrap();
        });
    });
}

fn bench_rebuild(c: &mut Criterion) {
    c.bench_function("search/rebuild", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let db_path = dir.path().join("index.db");
                let repo_dir = dir.path().join("repo");
                let repo = GitRepo::init(&repo_dir).unwrap();
                let files: Vec<(String, String)> = (0..DOOGAT_COUNT_1K)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo.commit_files(&refs, "seed").unwrap();
                let index = Index::open(&db_path).unwrap();
                (dir, repo, index)
            },
            |(_dir, repo, index)| {
                index.rebuild(&repo).unwrap();
            },
        );
    });
}

fn bench_incremental_reindex(c: &mut Criterion) {
    c.bench_function("search/incremental_reindex", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let db_path = dir.path().join("index.db");
                let repo_dir = dir.path().join("repo");
                let repo = GitRepo::init(&repo_dir).unwrap();
                let files: Vec<(String, String)> = (0..DOOGAT_COUNT_1K)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo.commit_files(&refs, "seed").unwrap();
                let index = Index::open(&db_path).unwrap();
                index.rebuild(&repo).unwrap();
                let old_head = index.stored_head_oid().unwrap();

                // Modify 1 doogat
                repo.commit_file(
                    &doogat_path(0),
                    &doogat_content(0).replace("Note about", "Modified note about"),
                    "modify one",
                )
                .unwrap();

                (dir, repo, index, old_head)
            },
            |(_dir, repo, index, old_head)| {
                index.incremental_reindex(&repo, &old_head).unwrap();
            },
        );
    });
}

fn bench_rebuild_5k(c: &mut Criterion) {
    c.bench_function("search/rebuild_5k", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let db_path = dir.path().join("index.db");
                let repo_dir = dir.path().join("repo");
                let repo = GitRepo::init(&repo_dir).unwrap();
                let files: Vec<(String, String)> = (0..DOOGAT_COUNT_5K)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo.commit_files(&refs, "seed").unwrap();
                let index = Index::open(&db_path).unwrap();
                (dir, repo, index)
            },
            |(_dir, repo, index)| {
                index.rebuild(&repo).unwrap();
            },
        );
    });
}

fn bench_incremental_reindex_batch(c: &mut Criterion) {
    c.bench_function("search/incremental_reindex_batch_10", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let db_path = dir.path().join("index.db");
                let repo_dir = dir.path().join("repo");
                let repo = GitRepo::init(&repo_dir).unwrap();
                let files: Vec<(String, String)> = (0..DOOGAT_COUNT_1K)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo.commit_files(&refs, "seed").unwrap();
                let index = Index::open(&db_path).unwrap();
                index.rebuild(&repo).unwrap();
                let old_head = index.stored_head_oid().unwrap();

                // Modify 10 doogats in a single commit
                let mods: Vec<(String, String)> = (0..10)
                    .map(|i| {
                        (
                            doogat_path(i),
                            doogat_content(i).replace("Note about", "Modified note about"),
                        )
                    })
                    .collect();
                let refs: Vec<(&str, &str)> =
                    mods.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
                repo.commit_files(&refs, "modify 10").unwrap();

                (dir, repo, index, old_head)
            },
            |(_dir, repo, index, old_head)| {
                index.incremental_reindex(&repo, &old_head).unwrap();
            },
        );
    });
}

criterion_group!(
    benches,
    bench_search,
    bench_query_raw,
    bench_search_5k,
    bench_query_raw_5k,
    bench_rebuild,
    bench_rebuild_5k,
    bench_incremental_reindex,
    bench_incremental_reindex_batch
);
criterion_main!(benches);
