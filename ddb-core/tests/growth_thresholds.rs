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
