use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{Result, DoogatError};
use crate::git_ops::GitRepo;
use crate::sync_manager::SyncManager;
use crate::types::{CompactOptions, CompactionReport};

/// Compute the default backup path for a pre-compaction bundle.
pub fn default_backup_path(repo: &GitRepo) -> PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    repo.path
        .join(".ddb/backups")
        .join(format!("pre-compact-{ts}.bundle.tar"))
}

/// Create a pre-compaction backup bundle.
/// Default path: `.ddb/backups/pre-compact-{ISO8601}.bundle.tar`
pub fn backup_before_compact(
    repo: &GitRepo,
    sync_mgr: &SyncManager,
    backup_path: Option<&Path>,
) -> Result<PathBuf> {
    let path = match backup_path {
        Some(p) => p.to_path_buf(),
        None => default_backup_path(repo),
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    crate::bundle::export_full_bundle(repo, sync_mgr, &path)
}

/// Find the greatest common ancestor commit reachable from all active nodes' known_heads.
/// Stale and retired nodes are excluded from the calculation.
pub fn shared_head(
    repo: &GitRepo,
    nodes: &[crate::types::NodeConfig],
) -> Result<Option<git2::Oid>> {
    let heads: Vec<git2::Oid> = nodes
        .iter()
        .filter(|n| n.status == crate::types::NodeStatus::Active)
        .filter_map(|n| n.known_heads.first())
        .filter_map(|h| git2::Oid::from_str(h).ok())
        .collect();

    if heads.is_empty() {
        return Ok(None);
    }
    if heads.len() == 1 {
        return Ok(Some(heads[0]));
    }

    // Iteratively compute merge-base across all heads
    let mut base = repo.repo.merge_base(heads[0], heads[1])?;
    for head in &heads[2..] {
        base = repo.repo.merge_base(base, *head)?;
    }

    Ok(Some(base))
}

/// Parse doogat ID from CRDT temp filename.
/// Supports formats: `{oid}_{doogat_id}.crdt`, `{oid}_{doogat_id}_fm.crdt`,
/// and legacy `{oid}` or `{oid}.crdt`.
/// Returns `(oid, doogat_id, is_frontmatter)`.
fn parse_crdt_temp_name(name: &str) -> Option<(git2::Oid, Option<String>, bool)> {
    let stem = name.strip_suffix(".crdt").unwrap_or(name);

    if let Some((oid_part, rest)) = stem.split_once('_') {
        let oid = git2::Oid::from_str(oid_part).ok()?;
        if let Some(doogat_id) = rest.strip_suffix("_fm") {
            Some((oid, Some(doogat_id.to_string()), true))
        } else {
            Some((oid, Some(rest.to_string()), false))
        }
    } else {
        let oid = git2::Oid::from_str(stem).ok()?;
        Some((oid, None, false))
    }
}

/// Remove temporary CRDT files older than the shared sync point.
pub fn cleanup_crdt_temp(repo: &GitRepo, shared_head: Option<git2::Oid>) -> Result<usize> {
    let temp_dir = repo.path.join(".crdt/temp");
    if !temp_dir.exists() {
        return Ok(0);
    }

    let Some(shared_head) = shared_head else {
        return Ok(0);
    };

    let mut removed = 0;
    for entry in std::fs::read_dir(&temp_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".gitkeep" {
            continue;
        }
        let Some((temp_commit_oid, _doogat_id, _is_fm)) = parse_crdt_temp_name(&name) else {
            continue;
        };

        if repo.repo.merge_base(shared_head, temp_commit_oid).ok() == Some(temp_commit_oid) {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }

    Ok(removed)
}

/// Compact CRDT temp files by grouping per doogat and merging Automerge changes.
/// Returns the number of doogats whose CRDT docs were compacted.
pub fn compact_crdt_docs(repo: &GitRepo) -> Result<usize> {
    let temp_dir = repo.path.join(".crdt/temp");
    if !temp_dir.exists() {
        return Ok(0);
    }

    // Group files by (doogat_id, is_frontmatter) so fm and body compact independently
    let mut by_key: HashMap<(String, bool), Vec<std::path::PathBuf>> = HashMap::new();
    for entry in std::fs::read_dir(&temp_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".gitkeep" {
            continue;
        }
        if let Some((_oid, Some(doogat_id), is_fm)) = parse_crdt_temp_name(&name) {
            by_key
                .entry((doogat_id, is_fm))
                .or_default()
                .push(entry.path());
        }
    }

    let mut compacted = 0;
    for ((doogat_id, is_fm), files) in &by_key {
        if files.len() < 2 {
            continue; // nothing to compact
        }

        // Load all Automerge changes and merge into a single doc
        let mut doc = automerge::AutoCommit::new();
        for file in files {
            if let Ok(data) = std::fs::read(file) {
                if let Ok(other) = automerge::AutoCommit::load(&data) {
                    doc.merge(&mut other.clone())
                        .map_err(|e| DoogatError::Automerge(e.to_string()))?;
                }
            }
        }

        // Save compacted doc with appropriate suffix
        let compacted_data = doc.save();
        let fm_suffix = if *is_fm { "_fm" } else { "" };
        let compacted_name = format!("compacted_{doogat_id}{fm_suffix}.crdt");
        std::fs::write(temp_dir.join(&compacted_name), compacted_data)?;

        // Remove original files
        for file in files {
            let _ = std::fs::remove_file(file);
        }

        compacted += 1;
    }

    Ok(compacted)
}

/// Compact CRDT docs for a single doogat.
pub fn compact_doogat(repo: &GitRepo, doogat_id: &str) -> Result<usize> {
    let temp_dir = repo.path.join(".crdt/temp");
    if !temp_dir.exists() {
        return Ok(0);
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&temp_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some((_oid, Some(zid), _is_fm)) = parse_crdt_temp_name(&name) {
            if zid == doogat_id {
                files.push(entry.path());
            }
        }
    }

    if files.len() < 2 {
        return Ok(0);
    }

    let mut doc = automerge::AutoCommit::new();
    for file in &files {
        if let Ok(data) = std::fs::read(file) {
            if let Ok(other) = automerge::AutoCommit::load(&data) {
                doc.merge(&mut other.clone())
                    .map_err(|e| DoogatError::Automerge(e.to_string()))?;
            }
        }
    }

    let compacted_data = doc.save();
    let compacted_name = format!("compacted_{doogat_id}.crdt");
    std::fs::write(temp_dir.join(&compacted_name), compacted_data)?;

    for file in &files {
        let _ = std::fs::remove_file(file);
    }

    Ok(1)
}

/// Get total size and file count of `.crdt/temp/` directory.
fn crdt_temp_stats(repo: &GitRepo) -> (u64, usize) {
    let temp_dir = repo.path.join(".crdt/temp");
    if !temp_dir.exists() {
        return (0, 0);
    }
    std::fs::read_dir(&temp_dir)
        .ok()
        .map(|entries| {
            let mut bytes = 0u64;
            let mut count = 0usize;
            for entry in entries.flatten() {
                if entry.file_name() == ".gitkeep" {
                    continue;
                }
                if let Ok(m) = entry.metadata() {
                    if m.is_file() {
                        bytes += m.len();
                        count += 1;
                    }
                }
            }
            (bytes, count)
        })
        .unwrap_or((0, 0))
}

/// Recursively compute total size of a directory in bytes.
fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(m) = entry.metadata() {
                if m.is_file() {
                    total += m.len();
                } else if m.is_dir() {
                    stack.push(entry.path());
                }
            }
        }
    }
    total
}

/// Run `git gc` on the repository.
pub fn run_gc(repo_path: &Path) -> Result<bool> {
    let output = std::process::Command::new("git")
        .args(["gc"])
        .current_dir(repo_path)
        .output()
        .map_err(DoogatError::Io)?;

    Ok(output.status.success())
}

/// Full compaction pipeline: threshold check → backup → shared head → cleanup → crdt doc compact → gc.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn compact(
    repo: &GitRepo,
    sync_mgr: &SyncManager,
    opts: &CompactOptions,
) -> Result<CompactionReport> {
    // Threshold check: skip if under threshold (unless forced)
    if !opts.force {
        let config = repo.load_config()?;
        let (crdt_bytes, crdt_files) = crdt_temp_stats(repo);
        let size_mb = crdt_bytes as f64 / (1024.0 * 1024.0);
        if size_mb < config.compaction.threshold_mb as f64 {
            let repo_bytes = dir_size(&repo.path.join(".git"));
            tracing::debug!(
                size_mb,
                threshold_mb = config.compaction.threshold_mb,
                "below_threshold_skip"
            );
            return Ok(CompactionReport {
                files_removed: 0,
                crdt_docs_compacted: 0,
                gc_success: true,
                crdt_temp_bytes_before: crdt_bytes,
                crdt_temp_bytes_after: crdt_bytes,
                crdt_temp_files_before: crdt_files,
                crdt_temp_files_after: crdt_files,
                repo_bytes_before: repo_bytes,
                repo_bytes_after: repo_bytes,
                backup_path: None,
            });
        }
    }

    // Pre-compaction backup
    let backup_path = if !opts.skip_backup {
        let bp = backup_before_compact(repo, sync_mgr, opts.backup_path.as_deref())?;
        tracing::info!(backup_path = %bp.display(), "pre_compaction_backup");
        Some(bp)
    } else {
        None
    };

    let (crdt_temp_bytes_before, crdt_temp_files_before) = crdt_temp_stats(repo);
    let repo_bytes_before = dir_size(&repo.path.join(".git"));

    let nodes = sync_mgr.list_nodes()?;
    let head = shared_head(repo, &nodes)?;
    tracing::debug!(shared_head = ?head, node_count = nodes.len(), "shared_head_computed");
    let files_removed = cleanup_crdt_temp(repo, head)?;
    if files_removed > 0 {
        tracing::info!(files_removed, "crdt_temp_cleanup");
    }

    let crdt_docs_compacted = compact_crdt_docs(repo)?;
    if crdt_docs_compacted > 0 {
        tracing::info!(crdt_docs_compacted, "crdt_docs_compacted");
    }

    let (crdt_temp_bytes_after, crdt_temp_files_after) = crdt_temp_stats(repo);

    let gc_success = run_gc(&repo.path)?;
    let repo_bytes_after = dir_size(&repo.path.join(".git"));

    tracing::info!(
        gc_success,
        crdt_temp_bytes_before,
        crdt_temp_bytes_after,
        repo_bytes_before,
        repo_bytes_after,
        "compaction_result"
    );

    crate::maintenance::maybe_auto_run(repo);

    Ok(CompactionReport {
        files_removed,
        crdt_docs_compacted,
        gc_success,
        crdt_temp_bytes_before,
        crdt_temp_bytes_after,
        crdt_temp_files_before,
        crdt_temp_files_after,
        repo_bytes_before,
        repo_bytes_after,
        backup_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use automerge::transaction::Transactable;

    fn temp_repo() -> (tempfile::TempDir, GitRepo) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        (dir, repo)
    }

    #[test]
    fn gc_runs_on_test_repo() {
        let (dir, _repo) = temp_repo();
        let success = run_gc(dir.path()).unwrap();
        assert!(success);
    }

    #[test]
    fn cleanup_empty_temp() {
        let (_dir, repo) = temp_repo();
        let removed = cleanup_crdt_temp(&repo, None).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn cleanup_removes_temp_files() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();
        let c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();
        let c3 = repo.commit_file("ddb/c.md", "c", "c3").unwrap();
        let temp_dir = repo.path.join(".crdt/temp");
        std::fs::write(temp_dir.join(c1.0.clone()), "data").unwrap();
        std::fs::write(temp_dir.join(c2.0.clone()), "data").unwrap();
        std::fs::write(temp_dir.join(c3.0.clone()), "data").unwrap();

        let c2_oid = git2::Oid::from_str(&c2.0).unwrap();
        let removed = cleanup_crdt_temp(&repo, Some(c2_oid)).unwrap();
        assert_eq!(removed, 2);
        assert!(!temp_dir.join(&c1.0).exists());
        assert!(!temp_dir.join(&c2.0).exists());
        assert!(temp_dir.join(&c3.0).exists());
    }

    #[test]
    fn cleanup_handles_new_naming_format() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();
        let c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();
        let temp_dir = repo.path.join(".crdt/temp");

        // New format: {oid}_{doogat_id}.crdt
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000.crdt", c1.0)),
            "data",
        )
        .unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120100.crdt", c2.0)),
            "data",
        )
        .unwrap();

        let c2_oid = git2::Oid::from_str(&c2.0).unwrap();
        let removed = cleanup_crdt_temp(&repo, Some(c2_oid)).unwrap();
        assert_eq!(removed, 2);
    }

    #[test]
    fn parse_crdt_temp_name_formats() {
        // Legacy: bare OID
        let (oid, zid, is_fm) =
            parse_crdt_temp_name("abc123def456abc123def456abc123def456abcd").unwrap();
        assert!(zid.is_none());
        assert!(!is_fm);
        assert_eq!(oid.to_string(), "abc123def456abc123def456abc123def456abcd");

        // Legacy: OID.crdt
        let (_, zid, is_fm) =
            parse_crdt_temp_name("abc123def456abc123def456abc123def456abcd.crdt").unwrap();
        assert!(zid.is_none());
        assert!(!is_fm);

        // New: OID_doogatid.crdt
        let (_, zid, is_fm) =
            parse_crdt_temp_name("abc123def456abc123def456abc123def456abcd_20260301120000.crdt")
                .unwrap();
        assert_eq!(zid.as_deref(), Some("20260301120000"));
        assert!(!is_fm);

        // Frontmatter: OID_doogatid_fm.crdt
        let (_, zid, is_fm) =
            parse_crdt_temp_name("abc123def456abc123def456abc123def456abcd_20260301120000_fm.crdt")
                .unwrap();
        assert_eq!(zid.as_deref(), Some("20260301120000"));
        assert!(is_fm);
    }

    #[test]
    fn compact_crdt_docs_groups_by_doogat() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();
        let temp_dir = repo.path.join(".crdt/temp");

        // Create dummy automerge docs for the same doogat
        let mut doc1 = automerge::AutoCommit::new();
        doc1.put(automerge::ROOT, "key", "val1").unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000.crdt", c1.0)),
            doc1.save(),
        )
        .unwrap();

        let mut doc2 = automerge::AutoCommit::new();
        doc2.put(automerge::ROOT, "key", "val2").unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000.crdt", c2.0)),
            doc2.save(),
        )
        .unwrap();

        let compacted = compact_crdt_docs(&repo).unwrap();
        assert_eq!(compacted, 1);

        // Should have one compacted file
        let files: Vec<_> = std::fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy() != ".gitkeep")
            .collect();
        assert_eq!(files.len(), 1);
        assert!(files[0]
            .file_name()
            .to_string_lossy()
            .starts_with("compacted_"));
    }

    #[test]
    fn compact_doogat_targets_single_doogat() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();
        let temp_dir = repo.path.join(".crdt/temp");

        // Doogat A: two files
        let mut doc = automerge::AutoCommit::new();
        doc.put(automerge::ROOT, "k", "v").unwrap();
        std::fs::write(temp_dir.join(format!("{}_A.crdt", c1.0)), doc.save()).unwrap();
        std::fs::write(temp_dir.join(format!("{}_A.crdt", c2.0)), doc.save()).unwrap();

        // Doogat B: one file (should not be touched)
        std::fs::write(temp_dir.join(format!("{}_B.crdt", c1.0)), doc.save()).unwrap();

        let compacted = compact_doogat(&repo, "A").unwrap();
        assert_eq!(compacted, 1);

        // B's file should still exist
        assert!(temp_dir.join(format!("{}_B.crdt", c1.0)).exists());
    }

    #[test]
    fn threshold_check_skips_when_under() {
        let (_dir, repo) = temp_repo();
        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        // No CRDT files → under threshold → should skip but still report actual stats
        let report = compact(
            &repo,
            &mgr,
            &CompactOptions {
                skip_backup: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(report.files_removed, 0);
        assert_eq!(report.crdt_docs_compacted, 0);
        assert!(report.gc_success);
        // Early return should still measure repo size (git dir exists)
        assert!(report.repo_bytes_before > 0);
        assert_eq!(report.repo_bytes_before, report.repo_bytes_after);
        // No CRDT temp files, so both before/after are zero
        assert_eq!(report.crdt_temp_bytes_before, 0);
        assert_eq!(report.crdt_temp_files_before, 0);
    }

    #[test]
    fn full_compact_pipeline() {
        let (_dir, repo) = temp_repo();
        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let report = compact(
            &repo,
            &mgr,
            &CompactOptions {
                force: true,
                skip_backup: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(report.gc_success);
        // Repo bytes should be measured (git dir exists)
        assert!(report.repo_bytes_before > 0 || report.repo_bytes_after > 0);
    }

    #[test]
    fn compact_with_backup() {
        let (_dir, repo) = temp_repo();
        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let report = compact(
            &repo,
            &mgr,
            &CompactOptions {
                force: true,
                skip_backup: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(report.gc_success);
        let bp = report
            .backup_path
            .expect("backup_path should be Some when skip_backup is false");
        assert!(bp.exists(), "backup file should exist at {}", bp.display());
        assert!(bp.to_string_lossy().contains("pre-compact-"));
    }

    #[test]
    fn compact_reduces_crdt_temp_bytes() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();

        // Create two CRDT temp files for the same doogat (will be compacted into one)
        let temp_dir = repo.path.join(".crdt/temp");
        let mut doc1 = automerge::AutoCommit::new();
        doc1.put(automerge::ROOT, "key", "value1").unwrap();
        let mut doc2 = automerge::AutoCommit::new();
        doc2.put(automerge::ROOT, "key", "value2").unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000.crdt", c1.0)),
            doc1.save(),
        )
        .unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000.crdt", c2.0)),
            doc2.save(),
        )
        .unwrap();

        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let report = compact(
            &repo,
            &mgr,
            &CompactOptions {
                force: true,
                skip_backup: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(report.gc_success);
        assert!(report.crdt_temp_bytes_before > 0);
        assert!(report.crdt_temp_files_before >= 2);
        // Two files compacted into one → fewer files and potentially fewer bytes
        assert!(report.crdt_temp_files_after < report.crdt_temp_files_before);
    }

    #[test]
    fn compact_crdt_docs_separates_fm_and_body() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();
        let temp_dir = repo.path.join(".crdt/temp");

        let mut doc = automerge::AutoCommit::new();
        doc.put(automerge::ROOT, "k", "v").unwrap();
        let bytes = doc.save();

        // Two body files for same doogat
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000.crdt", c1.0)),
            &bytes,
        )
        .unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000.crdt", c2.0)),
            &bytes,
        )
        .unwrap();
        // Two fm files for same doogat
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000_fm.crdt", c1.0)),
            &bytes,
        )
        .unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000_fm.crdt", c2.0)),
            &bytes,
        )
        .unwrap();

        let compacted = compact_crdt_docs(&repo).unwrap();
        // Should compact body and fm independently → 2 groups compacted
        assert_eq!(compacted, 2);

        let files: Vec<String> = std::fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != ".gitkeep")
            .collect();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f == "compacted_20260301120000.crdt"));
        assert!(files
            .iter()
            .any(|f| f == "compacted_20260301120000_fm.crdt"));
    }

    #[test]
    fn cleanup_handles_fm_naming_format() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();
        let c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();
        let temp_dir = repo.path.join(".crdt/temp");

        // Create _fm.crdt files
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000_fm.crdt", c1.0)),
            "data",
        )
        .unwrap();
        std::fs::write(
            temp_dir.join(format!("{}_20260301120000_fm.crdt", c2.0)),
            "data",
        )
        .unwrap();

        let c2_oid = git2::Oid::from_str(&c2.0).unwrap();
        let removed = cleanup_crdt_temp(&repo, Some(c2_oid)).unwrap();
        assert_eq!(removed, 2);
    }

    #[test]
    fn backup_before_compact_writes_file() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("ddb/a.md", "a", "c1").unwrap();
        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let out = repo.path.join("test-backup.bundle.tar");
        let result = backup_before_compact(&repo, &mgr, Some(&out)).unwrap();
        assert_eq!(result, out);
        assert!(out.exists());
        assert!(std::fs::metadata(&out).unwrap().len() > 0);
    }

    #[test]
    fn backup_before_compact_default_path() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("ddb/a.md", "a", "c1").unwrap();
        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let result = backup_before_compact(&repo, &mgr, None).unwrap();
        assert!(result.starts_with(repo.path.join(".ddb/backups")));
        assert!(result.to_string_lossy().contains("pre-compact-"));
        assert!(result.to_string_lossy().ends_with(".bundle.tar"));
        assert!(result.exists());
    }

    #[test]
    fn compact_skip_backup() {
        let (_dir, repo) = temp_repo();
        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let opts = CompactOptions {
            force: true,
            skip_backup: true,
            ..Default::default()
        };
        let report = compact(&repo, &mgr, &opts).unwrap();
        assert!(report.backup_path.is_none());
    }

    #[test]
    fn compact_backup_failure_aborts() {
        let (_dir, repo) = temp_repo();
        let c1 = repo.commit_file("ddb/a.md", "a", "c1").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let _c2 = repo.commit_file("ddb/b.md", "b", "c2").unwrap();

        // Create CRDT temp files that compaction would normally remove
        let temp_dir = repo.path.join(".crdt/temp");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let temp_file = temp_dir.join(format!("{}_20260101120000.crdt", c1.0));
        std::fs::write(&temp_file, "crdt-data").unwrap();

        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let (bytes_before, files_before) = crdt_temp_stats(&repo);

        // Use an existing file as the would-be parent directory so create_dir_all fails
        // consistently across Unix and Windows.
        let invalid_parent = repo.path.join("not-a-directory");
        std::fs::write(&invalid_parent, "x").unwrap();
        let opts = CompactOptions {
            force: true,
            skip_backup: false,
            backup_path: Some(invalid_parent.join("backup.bundle.tar")),
        };
        let result = compact(&repo, &mgr, &opts);
        assert!(result.is_err());

        // Verify no mutations occurred — CRDT temp files untouched
        let (bytes_after, files_after) = crdt_temp_stats(&repo);
        assert_eq!(bytes_before, bytes_after);
        assert_eq!(files_before, files_after);
        assert!(temp_file.exists());
    }

    #[test]
    fn frontmatter_crdt_preserved_when_newer_than_shared_head() {
        let (_dir, repo) = temp_repo();

        // Create two commits to serve as time anchors
        let commit_old = repo.commit_file("ddb/a.md", "a", "c_old").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let commit_new = repo.commit_file("ddb/b.md", "b", "c_new").unwrap();

        // Create a _fm.crdt file named with commit_new's OID
        let temp_dir = repo.path.join(".crdt/temp");
        let fm_crdt_name = format!("{}_20260301120000_fm.crdt", commit_new.0);
        std::fs::write(temp_dir.join(&fm_crdt_name), "fm-crdt-data").unwrap();

        // cleanup with shared_head = commit_old → file is NEWER, should NOT be removed
        let old_oid = git2::Oid::from_str(&commit_old.0).unwrap();
        let removed = cleanup_crdt_temp(&repo, Some(old_oid)).unwrap();
        assert_eq!(removed, 0, "file newer than shared_head should be preserved");
        assert!(
            temp_dir.join(&fm_crdt_name).exists(),
            "_fm.crdt file should still exist after cleanup with older shared_head"
        );

        // cleanup with shared_head = commit_new → file is at or older, should be removed
        let new_oid = git2::Oid::from_str(&commit_new.0).unwrap();
        let removed = cleanup_crdt_temp(&repo, Some(new_oid)).unwrap();
        assert_eq!(removed, 1, "file at shared_head should be removed");
        assert!(
            !temp_dir.join(&fm_crdt_name).exists(),
            "_fm.crdt file should be removed after cleanup with matching shared_head"
        );
    }

    #[test]
    fn compact_under_threshold_no_backup() {
        let (_dir, repo) = temp_repo();
        crate::sync_manager::register_node(&repo, "Test").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        // Under threshold, backup should not run
        let opts = CompactOptions {
            skip_backup: false,
            ..Default::default()
        };
        let report = compact(&repo, &mgr, &opts).unwrap();
        assert!(report.backup_path.is_none());
    }
}
