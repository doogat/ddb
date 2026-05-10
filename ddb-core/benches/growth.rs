use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use ddb_core::compaction;
use ddb_core::git_ops::GitRepo;
use ddb_core::sync_manager::{register_node, SyncManager};
use tempfile::TempDir;

include!("helpers.rs");

/// Simulate repo growth: create doogats, then modify them over time.
/// NFR-02 / AC-08: repo growth < 50MB/year at 5K doogats.
///
/// Strategy: start with 5K doogats, then simulate 365 days of edits
/// (10 modifications/day = 3650 commits) and measure repo size.
/// Using 10/day instead of 100/day to keep bench runtime reasonable;
/// the threshold is scaled proportionally.
const INITIAL_DOOGATS: usize = 5000;
const DAYS: usize = 365;
const EDITS_PER_DAY: usize = 10;
const DAYS_PER_MONTH: usize = 30;

struct GrowthSnapshot {
    month: usize,
    git_bytes: u64,
    crdt_temp_bytes: u64,
    total_bytes: u64,
}

fn git_dir_size(repo_path: &std::path::Path) -> u64 {
    dir_size(&repo_path.join(".git"))
}

fn crdt_temp_dir_size(repo_path: &std::path::Path) -> u64 {
    dir_size(&repo_path.join(".crdt/temp"))
}

fn simulate_year(repo: &GitRepo, with_compaction: bool) -> Vec<GrowthSnapshot> {
    let mgr = if with_compaction {
        Some(SyncManager::open(repo).unwrap())
    } else {
        None
    };

    let mut snapshots = Vec::new();

    for day in 0..DAYS {
        let batch: Vec<(String, String)> = (0..EDITS_PER_DAY)
            .map(|edit| {
                let idx = (day * EDITS_PER_DAY + edit) % INITIAL_DOOGATS;
                let content = format!(
                    "---\ntitle: Updated note {idx} day {day}\ndate: 2026-01-01\ntags:\n  - bench\n---\n\
                     Modified on day {day}, edit {edit}.\n\
                     ---\n- source:: bench-{idx}"
                );
                (doogat_path(idx), content)
            })
            .collect();
        let refs: Vec<(&str, &str)> = batch
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        repo.commit_files(&refs, &format!("day {day}")).unwrap();

        // Monthly snapshot
        if (day + 1) % DAYS_PER_MONTH == 0 {
            let month = (day + 1) / DAYS_PER_MONTH;

            if let Some(ref mgr) = mgr {
                let opts = ddb_core::types::CompactOptions {
                    force: true,
                    skip_backup: true,
                    ..Default::default()
                };
                let _ = compaction::compact(repo, mgr, &opts);
            }

            snapshots.push(GrowthSnapshot {
                month,
                git_bytes: git_dir_size(&repo.path),
                crdt_temp_bytes: crdt_temp_dir_size(&repo.path),
                total_bytes: dir_size(&repo.path),
            });
        }
    }

    snapshots
}

fn bench_growth(c: &mut Criterion) {
    let mut group = c.benchmark_group("growth");
    // Only run once — this is a measurement, not a hot-path benchmark
    group.sample_size(10);

    group.bench_function("repo_size_after_1yr", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let repo = GitRepo::init(dir.path()).unwrap();

                // Seed with initial doogats
                let files: Vec<(String, String)> = (0..INITIAL_DOOGATS)
                    .map(|i| (doogat_path(i), doogat_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                repo.commit_files(&refs, "seed").unwrap();

                (dir, repo)
            },
            |(dir, repo)| {
                let size_before = dir_size(dir.path());
                let snapshots = simulate_year(&repo, false);
                let size_after = dir_size(dir.path());
                let growth = size_after - size_before;

                // Print monthly breakdown on first run
                if !snapshots.is_empty() {
                    eprintln!("\n  [no-compaction] monthly sizes:");
                    for s in &snapshots {
                        eprintln!(
                            "    month {:2}: total={:.1}MB git={:.1}MB crdt_temp={:.1}MB",
                            s.month,
                            s.total_bytes as f64 / 1_048_576.0,
                            s.git_bytes as f64 / 1_048_576.0,
                            s.crdt_temp_bytes as f64 / 1_048_576.0,
                        );
                    }
                    eprintln!("    growth: {:.1}MB", growth as f64 / 1_048_576.0);
                }

                black_box(growth);
            },
        );
    });

    group.bench_function("repo_size_after_1yr_with_compaction", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let repo = GitRepo::init(dir.path()).unwrap();

                let files: Vec<(String, String)> = (0..INITIAL_DOOGATS)
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
            |(dir, repo)| {
                let size_before = dir_size(dir.path());
                let snapshots = simulate_year(&repo, true);
                let size_after = dir_size(dir.path());
                let growth = size_after - size_before;

                if !snapshots.is_empty() {
                    eprintln!("\n  [with-compaction] monthly sizes:");
                    for s in &snapshots {
                        eprintln!(
                            "    month {:2}: total={:.1}MB git={:.1}MB crdt_temp={:.1}MB",
                            s.month,
                            s.total_bytes as f64 / 1_048_576.0,
                            s.git_bytes as f64 / 1_048_576.0,
                            s.crdt_temp_bytes as f64 / 1_048_576.0,
                        );
                    }
                    eprintln!("    growth: {:.1}MB", growth as f64 / 1_048_576.0);
                }

                black_box(growth);
            },
        );
    });

    group.finish();
}

criterion_group!(benches, bench_growth);
criterion_main!(benches);
