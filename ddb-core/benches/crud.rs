use std::path::Path;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use ddb_core::git_ops::GitRepo;
use tempfile::TempDir;

const DOOGAT_COUNT: usize = 1000;

fn doogat_content(i: usize) -> String {
    format!(
        "---\ntitle: Note {i}\ndate: 2026-01-01\ntags:\n  - bench\n---\nBody of doogat {i}.\n---\n- source:: bench-{i}"
    )
}

fn doogat_path(i: usize) -> String {
    format!("ddb/{:014}.md", 20260101000000u64 + i as u64)
}

fn populated_repo(dir: &Path) -> GitRepo {
    let repo = GitRepo::init(dir).unwrap();
    let files: Vec<(String, String)> = (0..DOOGAT_COUNT)
        .map(|i| (doogat_path(i), doogat_content(i)))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    repo.commit_files(&refs, "seed").unwrap();
    repo
}

fn bench_create(c: &mut Criterion) {
    c.bench_function("crud/create", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().unwrap();
                let repo = populated_repo(dir.path());
                (dir, repo)
            },
            |(_dir, repo)| {
                let path = format!("ddb/{:014}.md", 20260101000000u64 + DOOGAT_COUNT as u64);
                repo.commit_file(&path, &doogat_content(DOOGAT_COUNT), "create")
                    .unwrap();
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_read(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let repo = populated_repo(dir.path());
    let target = doogat_path(500);

    c.bench_function("crud/read", |b| {
        b.iter(|| {
            repo.read_file(&target).unwrap();
        });
    });
}

fn bench_update(c: &mut Criterion) {
    c.bench_function("crud/update", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().unwrap();
                let repo = populated_repo(dir.path());
                (dir, repo)
            },
            |(_dir, repo)| {
                let path = doogat_path(500);
                repo.commit_file(&path, "---\ntitle: Updated\n---\nNew body.", "update")
                    .unwrap();
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_delete(c: &mut Criterion) {
    c.bench_function("crud/delete", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().unwrap();
                let repo = populated_repo(dir.path());
                (dir, repo)
            },
            |(_dir, repo)| {
                let path = doogat_path(500);
                repo.delete_file(&path, "delete").unwrap();
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_batch_commit(c: &mut Criterion) {
    c.bench_function("crud/batch_commit_10", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().unwrap();
                let repo = populated_repo(dir.path());
                (dir, repo)
            },
            |(_dir, repo)| {
                let files: Vec<(String, String)> = (DOOGAT_COUNT..DOOGAT_COUNT + 10)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo.commit_files(&refs, "batch").unwrap();
            },
            BatchSize::PerIteration,
        );
    });
}

criterion_group!(
    benches,
    bench_create,
    bench_read,
    bench_update,
    bench_delete,
    bench_batch_commit
);
criterion_main!(benches);
