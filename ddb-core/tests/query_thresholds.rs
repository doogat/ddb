use std::time::Instant;

use tempfile::TempDir;
use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;

const DOOGAT_COUNT_5K: usize = 5000;
const DOOGAT_COUNT_50K: usize = 50_000;
const NFR01_THRESHOLD_MS: u128 = 10;
const AC19_THRESHOLD_MS: u128 = 50;
const WARMUP_ITERS: usize = 3;
const MEASURE_ITERS: usize = 10;

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

fn setup(count: usize) -> (TempDir, Index) {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    let db_path = dir.path().join("index.db");

    let repo = GitRepo::init(&repo_dir).unwrap();
    let batch_size = 5000;
    for start in (0..count).step_by(batch_size) {
        let end = (start + batch_size).min(count);
        let files: Vec<(String, String)> = (start..end)
            .map(|i| (doogat_path(i), doogat_content(i)))
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        repo.commit_files(&refs, &format!("batch {start}")).unwrap();
    }

    let index = Index::open(&db_path).unwrap();
    index.rebuild(&repo).unwrap();
    (dir, index)
}

fn median_ms<F: FnMut()>(mut f: F) -> u128 {
    // warmup
    for _ in 0..WARMUP_ITERS {
        f();
    }
    // measure
    let mut times = Vec::with_capacity(MEASURE_ITERS);
    for _ in 0..MEASURE_ITERS {
        let start = Instant::now();
        f();
        times.push(start.elapsed().as_millis());
    }
    times.sort();
    times[MEASURE_ITERS / 2]
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "performance thresholds require --release; debug CI runners are too noisy"
)]
fn nfr01_fts_query_under_10ms_at_5k() {
    let (_dir, index) = setup(DOOGAT_COUNT_5K);
    let ms = median_ms(|| {
        index.search("architecture").unwrap();
    });
    assert!(
        ms < NFR01_THRESHOLD_MS,
        "NFR-01: FTS query took {ms}ms, threshold is {NFR01_THRESHOLD_MS}ms"
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "performance thresholds require --release; debug CI runners are too noisy"
)]
fn nfr01_sql_query_under_10ms_at_5k() {
    let (_dir, index) = setup(DOOGAT_COUNT_5K);
    let ms = median_ms(|| {
        index
            .query_raw("SELECT id, title FROM doogats WHERE title LIKE '%architecture%' LIMIT 10")
            .unwrap();
    });
    assert!(
        ms < NFR01_THRESHOLD_MS,
        "NFR-01: SQL query took {ms}ms, threshold is {NFR01_THRESHOLD_MS}ms"
    );
}

/// AC-19: query < 50ms at 50K doogats. Long setup (~minutes).
/// Run with: cargo test --release --test query_thresholds -- --ignored
#[test]
#[ignore = "50K setup takes minutes — run explicitly"]
fn ac19_fts_query_under_50ms_at_50k() {
    let (_dir, index) = setup(DOOGAT_COUNT_50K);
    let ms = median_ms(|| {
        index.search("architecture").unwrap();
    });
    assert!(
        ms < AC19_THRESHOLD_MS,
        "AC-19: FTS query took {ms}ms, threshold is {AC19_THRESHOLD_MS}ms"
    );
}

#[test]
#[ignore = "50K setup takes minutes — run explicitly"]
fn ac19_sql_query_under_50ms_at_50k() {
    let (_dir, index) = setup(DOOGAT_COUNT_50K);
    let ms = median_ms(|| {
        index
            .query_raw("SELECT id, title FROM doogats WHERE title LIKE '%architecture%' LIMIT 10")
            .unwrap();
    });
    assert!(
        ms < AC19_THRESHOLD_MS,
        "AC-19: SQL query took {ms}ms, threshold is {AC19_THRESHOLD_MS}ms"
    );
}
