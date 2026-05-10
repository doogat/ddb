use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};
use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use tempfile::TempDir;

/// AC-19: query < 50ms at 50K doogats.
/// Separate benchmark target due to long setup time.
const DOOGAT_COUNT: usize = 50_000;

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
         ---\n- source:: bench-{i}"
    )
}

fn doogat_path(i: usize) -> String {
    format!("ddb/{:014}.md", 20260101000000u64 + i as u64)
}

fn populated_repo_and_index(repo_dir: &Path, db_path: &Path) -> (GitRepo, Index) {
    let repo = GitRepo::init(repo_dir).unwrap();

    // Commit in batches to avoid excessive memory usage
    let batch_size = 5000;
    for start in (0..DOOGAT_COUNT).step_by(batch_size) {
        let end = (start + batch_size).min(DOOGAT_COUNT);
        let files: Vec<(String, String)> = (start..end)
            .map(|i| (doogat_path(i), doogat_content(i)))
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        repo.commit_files(&refs, &format!("batch {start}")).unwrap();
    }

    let index = Index::open(db_path).unwrap();
    index.rebuild(&repo).unwrap();
    (repo, index)
}

fn bench_fts_50k(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("index.db");
    let repo_dir = dir.path().join("repo");
    let (_repo, index) = populated_repo_and_index(&repo_dir, &db_path);

    let mut group = c.benchmark_group("large_scale");
    group.sample_size(20);

    group.bench_function("fts_50k", |b| {
        b.iter(|| {
            index.search("architecture").unwrap();
        });
    });

    group.bench_function("sql_select_50k", |b| {
        b.iter(|| {
            index
                .query_raw(
                    "SELECT id, title FROM doogats WHERE title LIKE '%architecture%' LIMIT 10",
                )
                .unwrap();
        });
    });

    group.finish();
}

criterion_group!(benches, bench_fts_50k);
criterion_main!(benches);
