use tempfile::TempDir;
use ddb_core::git_ops::GitRepo;

include!("../benches/helpers.rs");

const INITIAL_DOOGATS: usize = 5000;
const DAYS: usize = 365;
const EDITS_PER_DAY: usize = 10;
const GROWTH_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024; // 50MB

/// NFR-02 / AC-08: repo growth < 50MB/year at 5K doogats.
/// Run with: cargo test --release --test growth_thresholds
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "growth thresholds require --release; debug runs are too slow"
)]
fn nfr02_repo_growth_under_50mb_per_year_at_5k() {
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

    let size_before = dir_size(dir.path());

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
    }

    let size_after = dir_size(dir.path());
    let growth = size_after - size_before;

    assert!(
        growth < GROWTH_THRESHOLD_BYTES,
        "NFR-02: repo grew {:.1}MB, threshold is {:.1}MB",
        growth as f64 / (1024.0 * 1024.0),
        GROWTH_THRESHOLD_BYTES as f64 / (1024.0 * 1024.0),
    );
}

const INITIAL_DOOGATS_50K: usize = 50_000;
const GROWTH_THRESHOLD_BYTES_50K: u64 = 200 * 1024 * 1024; // 200MB (spec NFR-02 50K target)

/// NFR-02 at 50K scale: repo growth < 200MB/year at 50K doogats.
/// Run with: cargo test --release --test growth_thresholds nfr02_50k
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "growth thresholds require --release; debug runs are too slow"
)]
fn nfr02_repo_growth_under_200mb_per_year_at_50k() {
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.set_skip_commit_graph(true);

    let files: Vec<(String, String)> = (0..INITIAL_DOOGATS_50K)
        .map(|i| (doogat_path(i), doogat_content(i)))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    repo.commit_files(&refs, "seed").unwrap();

    // Pack seed objects so size_before reflects packed baseline.
    assert!(
        std::process::Command::new("git")
            .args(["gc"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let size_before = dir_size(dir.path());

    for day in 0..DAYS {
        let batch: Vec<(String, String)> = (0..EDITS_PER_DAY)
            .map(|edit| {
                let idx = (day * EDITS_PER_DAY + edit) % INITIAL_DOOGATS_50K;
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
    }

    // Repack before measuring. In production, maintenance runs from multiple
    // triggers (write-threshold, startup, ddb compact). Without packing,
    // loose 50K-entry tree objects dominate disk usage.
    assert!(
        std::process::Command::new("git")
            .args(["gc"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );

    let size_after = dir_size(dir.path());
    let growth = size_after - size_before;

    assert!(
        growth < GROWTH_THRESHOLD_BYTES_50K,
        "NFR-02 (50K): repo grew {:.1}MB, threshold is {:.1}MB",
        growth as f64 / (1024.0 * 1024.0),
        GROWTH_THRESHOLD_BYTES_50K as f64 / (1024.0 * 1024.0),
    );
}
