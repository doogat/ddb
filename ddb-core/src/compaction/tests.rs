
use super::*;
use crate::git_ops::GitRepo;
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

    let removed = cleanup_crdt_temp(&repo, Some(&c2.0)).unwrap();
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

    let removed = cleanup_crdt_temp(&repo, Some(&c2.0)).unwrap();
    assert_eq!(removed, 2);
}

#[test]
fn parse_crdt_temp_name_formats() {
    // Legacy: bare OID
    let (oid, zid, is_fm) =
        parse_crdt_temp_name("abc123def456abc123def456abc123def456abcd").unwrap();
    assert!(zid.is_none());
    assert!(!is_fm);
    assert_eq!(oid, "abc123def456abc123def456abc123def456abcd");

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

    let removed = cleanup_crdt_temp(&repo, Some(&c2.0)).unwrap();
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

    // cleanup with shared_head = commit_old - file is NEWER, should NOT be removed
    let removed = cleanup_crdt_temp(&repo, Some(&commit_old.0)).unwrap();
    assert_eq!(
        removed, 0,
        "file newer than shared_head should be preserved"
    );
    assert!(
        temp_dir.join(&fm_crdt_name).exists(),
        "_fm.crdt file should still exist after cleanup with older shared_head"
    );

    // cleanup with shared_head = commit_new - file is at or older, should be removed
    let removed = cleanup_crdt_temp(&repo, Some(&commit_new.0)).unwrap();
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
