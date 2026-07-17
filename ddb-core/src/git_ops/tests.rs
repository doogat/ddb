use super::*;
use tempfile::TempDir;

fn temp_repo() -> (TempDir, GitRepo) {
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    (dir, repo)
}

fn native_absolute_path() -> &'static str {
    if cfg!(windows) {
        r"C:\Windows\System32\drivers\etc\hosts"
    } else {
        "/etc/passwd"
    }
}

#[test]
fn init_creates_directory_structure() {
    let (dir, _repo) = temp_repo();
    assert!(dir.path().join("ddb/.gitkeep").exists());
    assert!(dir.path().join("reference/.gitkeep").exists());
    assert!(dir.path().join(".nodes/.gitkeep").exists());
    assert!(dir.path().join(".crdt/temp/.gitkeep").exists());
}

#[test]
fn init_creates_gitignore() {
    let (dir, _repo) = temp_repo();
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains(".ddb/"));
}

#[test]
fn init_creates_initial_commit() {
    let (_dir, repo) = temp_repo();
    let head = repo.head_oid();
    assert!(head.is_ok());
}

#[test]
fn open_existing_repo() {
    let (dir, _repo) = temp_repo();
    let reopened = GitRepo::open(dir.path());
    assert!(reopened.is_ok());
}

#[test]
fn commit_and_read_file() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/test.md", "hello world", "add test")
        .unwrap();
    let content = repo.read_file("ddb/test.md").unwrap();
    assert_eq!(content, "hello world");
}

#[test]
fn commit_binary_file_roundtrip() {
    let (_dir, repo) = temp_repo();
    let bytes: Vec<u8> = (0..=255).collect();
    repo.commit_binary_file("reference/test/blob.bin", &bytes, "add binary")
        .unwrap();
    let full = _dir.path().join("reference/test/blob.bin");
    let read_back = std::fs::read(full).unwrap();
    assert_eq!(read_back, bytes);
}

#[test]
fn commit_multiple_files() {
    let (_dir, repo) = temp_repo();
    repo.commit_files(&[("ddb/a.md", "aaa"), ("ddb/b.md", "bbb")], "add two files")
        .unwrap();
    assert_eq!(repo.read_file("ddb/a.md").unwrap(), "aaa");
    assert_eq!(repo.read_file("ddb/b.md").unwrap(), "bbb");
}

#[test]
fn read_file_not_found() {
    let (_dir, repo) = temp_repo();
    let result = repo.read_file("nonexistent.md");
    assert!(result.is_err());
}

#[test]
fn read_files_batch_matches_individual() {
    let (_dir, repo) = temp_repo();
    let paths: Vec<String> = (0..20)
        .map(|i| format!("ddb/{:014}.md", 20260101000000u64 + i))
        .collect();
    for (i, path) in paths.iter().enumerate() {
        repo.commit_file(path, &format!("content {i}"), &format!("add {i}"))
            .unwrap();
    }

    let batch = repo.read_files_batch(&paths).unwrap();
    assert_eq!(batch.len(), paths.len());
    for (path, result) in &batch {
        let expected = repo.read_file(path).unwrap();
        assert_eq!(result.as_ref().unwrap(), &expected);
    }
}

#[test]
fn read_files_batch_partial_errors() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/a.md", "content a", "add a").unwrap();
    let paths = vec!["ddb/a.md".to_string(), "ddb/missing.md".to_string()];
    let batch = repo.read_files_batch(&paths).unwrap();
    assert!(batch[0].1.is_ok());
    assert!(batch[1].1.is_err());
}

#[test]
fn list_doogats_finds_md_files() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/a.md", "a", "add a").unwrap();
    repo.commit_file("ddb/sub/b.md", "b", "add b").unwrap();
    repo.commit_file("reference/c.md", "c", "add c").unwrap();

    let doogats = repo.list_doogats().unwrap();
    assert_eq!(doogats.len(), 2);
    assert!(doogats.iter().any(|p| p == "ddb/a.md"));
    assert!(doogats.iter().any(|p| p == "ddb/sub/b.md"));
}

#[test]
fn init_creates_version_file() {
    let (dir, _repo) = temp_repo();
    let content = std::fs::read_to_string(dir.path().join(".ddb-version")).unwrap();
    assert_eq!(content.trim(), "1");
}

#[test]
fn open_succeeds_on_matching_version() {
    let (dir, _repo) = temp_repo();
    let reopened = GitRepo::open(dir.path());
    assert!(reopened.is_ok());
}

#[test]
fn open_rejects_higher_version() {
    let (dir, _repo) = temp_repo();
    // Commit a version file with a future version
    {
        std::fs::write(dir.path().join(".ddb-version"), "999").unwrap();
        let raw_repo = Repository::open(dir.path()).unwrap();
        let sig = Signature::now("ddb", "ddb@local").unwrap();
        let mut index = raw_repo.index().unwrap();
        index.add_path(Path::new(".ddb-version")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = raw_repo.find_tree(tree_oid).unwrap();
        let parent = raw_repo.head().unwrap().peel_to_commit().unwrap();
        raw_repo
            .commit(Some("HEAD"), &sig, &sig, "bump version", &tree, &[&parent])
            .unwrap();
    }

    let err = GitRepo::open(dir.path()).err().expect("should fail");
    assert!(format!("{err}").contains("version mismatch"));
}

#[test]
fn init_creates_config_file() {
    let (dir, _repo) = temp_repo();
    assert!(dir.path().join(".ddb.toml").exists());
}

#[test]
fn load_config_returns_defaults() {
    let (_dir, repo) = temp_repo();
    let config = repo.load_config().unwrap();
    assert_eq!(config.compaction.stale_ttl_days, 90);
    assert_eq!(config.compaction.threshold_mb, 1);
    assert_eq!(config.crdt.default_strategy, "preset:default");
}

#[test]
fn open_cleans_orphaned_crdt_temp() {
    let (dir, _repo) = temp_repo();
    let temp_dir = dir.path().join(".crdt/temp");
    std::fs::write(temp_dir.join("orphan1.crdt"), "data").unwrap();
    std::fs::write(temp_dir.join("orphan2"), "data").unwrap();

    // Reopen — should clean up orphans but keep .gitkeep
    let _repo = GitRepo::open(dir.path()).unwrap();
    assert!(!temp_dir.join("orphan1.crdt").exists());
    assert!(!temp_dir.join("orphan2").exists());
    assert!(temp_dir.join(".gitkeep").exists());
}

#[test]
fn load_config_custom_values() {
    let (_dir, repo) = temp_repo();
    let custom = "[compaction]\nstale_ttl_days = 30\nthreshold_mb = 5\n";
    repo.commit_file(".ddb.toml", custom, "custom config")
        .unwrap();
    let config = repo.load_config().unwrap();
    assert_eq!(config.compaction.stale_ttl_days, 30);
    assert_eq!(config.compaction.threshold_mb, 5);
    // crdt section missing → defaults
    assert_eq!(config.crdt.default_strategy, "preset:default");
}

#[test]
fn open_auto_upgrades_pre_version_repo() {
    // Create a repo without version file (simulating v0)
    let dir = TempDir::new().unwrap();
    {
        let raw_repo = Repository::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("ddb")).unwrap();
        std::fs::write(dir.path().join("ddb/.gitkeep"), "").unwrap();
        let sig = Signature::now("ddb", "ddb@local").unwrap();
        let mut index = raw_repo.index().unwrap();
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = raw_repo.find_tree(tree_oid).unwrap();
        raw_repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
    }

    // open() should auto-upgrade
    let repo = GitRepo::open(dir.path()).unwrap();
    let content = repo.read_file(VERSION_FILE).unwrap();
    assert_eq!(content.trim(), "1");
}

fn setup_two_repos() -> (TempDir, GitRepo, TempDir, GitRepo, TempDir) {
    // Bare remote
    let bare_dir = TempDir::new().unwrap();
    Repository::init_bare(bare_dir.path()).unwrap();

    // Repo A
    let dir_a = TempDir::new().unwrap();
    let repo_a = GitRepo::init(dir_a.path()).unwrap();
    repo_a
        .add_remote("origin", bare_dir.path().to_str().unwrap())
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Repo B (clone)
    let dir_b = TempDir::new().unwrap();
    let repo_b_raw = Repository::clone(bare_dir.path().to_str().unwrap(), dir_b.path()).unwrap();
    drop(repo_b_raw);
    let repo_b = GitRepo::open(dir_b.path()).unwrap();

    (dir_a, repo_a, dir_b, repo_b, bare_dir)
}

#[test]
fn push_and_fetch_cycle() {
    let (_da, repo_a, _db, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/test.md", "hello", "add test")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b.fetch("origin", "master").unwrap();
    let result = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(result, MergeResult::FastForward(_)));

    let content = repo_b.read_file("ddb/test.md").unwrap();
    assert_eq!(content, "hello");
}

#[test]
fn merge_already_up_to_date() {
    let (_da, _repo_a, _db, repo_b, _bare) = setup_two_repos();
    repo_b.fetch("origin", "master").unwrap();
    let result = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(result, MergeResult::AlreadyUpToDate));
}

#[test]
fn merge_detects_conflicts() {
    let (_da, repo_a, _db, repo_b, _bare) = setup_two_repos();

    // Both create same file with different content
    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    match result {
        MergeResult::Conflicts(conflicts, _theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].path, "ddb/note.md");
            assert!(conflicts[0].ours.contains("version B"));
            assert!(conflicts[0].theirs.contains("version A"));
        }
        other => panic!("expected Conflicts, got {:?}", other),
    }
}

#[test]
fn delete_files_removes_multiple() {
    let (_dir, repo) = temp_repo();
    repo.commit_files(&[("ddb/a.md", "aaa"), ("ddb/b.md", "bbb")], "add two")
        .unwrap();
    repo.delete_files(&["ddb/a.md", "ddb/b.md"], "remove both")
        .unwrap();
    assert!(repo.read_file("ddb/a.md").is_err());
    assert!(repo.read_file("ddb/b.md").is_err());
}

#[test]
fn commit_batch_writes_and_deletes() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/old.md", "old content", "add old")
        .unwrap();
    repo.commit_batch(
        &[("ddb/new.md", "new content")],
        &["ddb/old.md"],
        "batch op",
    )
    .unwrap();
    assert_eq!(repo.read_file("ddb/new.md").unwrap(), "new content");
    assert!(repo.read_file("ddb/old.md").is_err());
}

#[test]
#[cfg(unix)]
fn symlink_read_rejected() {
    let (dir, repo) = temp_repo();
    repo.commit_file("ddb/real.md", "content", "add").unwrap();
    // Create a symlink on disk pointing to the real file
    let link = dir.path().join("ddb/link.md");
    std::os::unix::fs::symlink(dir.path().join("ddb/real.md"), &link).unwrap();
    let err = repo.read_file("ddb/link.md").unwrap_err();
    assert!(matches!(err, DoogatError::InvalidPath(_)));
}

#[test]
fn dotdot_path_rejected() {
    let (_dir, repo) = temp_repo();
    let err = repo.read_file("ddb/../../etc/passwd").unwrap_err();
    assert!(matches!(err, DoogatError::InvalidPath(_)));
}

#[test]
fn normal_path_accepted() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/normal.md", "ok", "add").unwrap();
    assert_eq!(repo.read_file("ddb/normal.md").unwrap(), "ok");
}

#[test]
#[cfg(unix)]
fn symlink_write_rejected() {
    let (dir, repo) = temp_repo();
    repo.commit_file("ddb/real.md", "original", "add").unwrap();
    let link = dir.path().join("ddb/link.md");
    std::os::unix::fs::symlink(dir.path().join("ddb/real.md"), &link).unwrap();
    let err = repo
        .commit_file("ddb/link.md", "hacked", "overwrite")
        .unwrap_err();
    assert!(matches!(err, DoogatError::InvalidPath(_)));
    // Original file unchanged
    assert_eq!(repo.read_file("ddb/real.md").unwrap(), "original");
}

#[test]
fn absolute_path_write_rejected() {
    let (_dir, repo) = temp_repo();
    let err = repo
        .commit_file(native_absolute_path(), "hacked", "write outside repo")
        .unwrap_err();
    assert!(matches!(err, DoogatError::InvalidPath(_)));
}

#[test]
fn absolute_path_read_rejected() {
    let (_dir, repo) = temp_repo();
    let err = repo.read_file(native_absolute_path()).unwrap_err();
    assert!(matches!(err, DoogatError::InvalidPath(_)));
}

#[test]
fn diff_paths_detects_added_modified_deleted() {
    let (_dir, repo) = temp_repo();

    // Create initial doogat and record HEAD
    repo.commit_file(
        "ddb/20240101000000.md",
        "---\ntitle: A\n---\nBody A.",
        "add a",
    )
    .unwrap();
    repo.commit_file(
        "ddb/20240102000000.md",
        "---\ntitle: B\n---\nBody B.",
        "add b",
    )
    .unwrap();
    let old_head = repo.head_oid().unwrap().to_string();

    // Modify one, delete one, add one
    repo.commit_file(
        "ddb/20240101000000.md",
        "---\ntitle: A modified\n---\nBody A modified.",
        "modify a",
    )
    .unwrap();
    repo.delete_file("ddb/20240102000000.md", "delete b")
        .unwrap();
    repo.commit_file(
        "ddb/20240103000000.md",
        "---\ntitle: C\n---\nBody C.",
        "add c",
    )
    .unwrap();
    let new_head = repo.head_oid().unwrap().to_string();

    let changes = repo.diff_paths(&old_head, &new_head).unwrap();
    assert_eq!(changes.len(), 3);

    use crate::types::DiffKind;
    let modified = changes
        .iter()
        .find(|(_, p)| p.contains("20240101"))
        .unwrap();
    assert_eq!(modified.0, DiffKind::Modified);
    let deleted = changes
        .iter()
        .find(|(_, p)| p.contains("20240102"))
        .unwrap();
    assert_eq!(deleted.0, DiffKind::Deleted);
    let added = changes
        .iter()
        .find(|(_, p)| p.contains("20240103"))
        .unwrap();
    assert_eq!(added.0, DiffKind::Added);
}

#[test]
fn diff_paths_ignores_non_doogat_files() {
    let (_dir, repo) = temp_repo();
    let old_head = repo.head_oid().unwrap().to_string();

    repo.commit_file(
        "ddb/20240101000000.md",
        "---\ntitle: Z\n---\n",
        "add doogat",
    )
    .unwrap();
    repo.commit_file("README.md", "# Hello", "add readme")
        .unwrap();
    let new_head = repo.head_oid().unwrap().to_string();

    let changes = repo.diff_paths(&old_head, &new_head).unwrap();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].1.contains("20240101"));
}

#[test]
fn diff_paths_unreachable_oid_returns_error() {
    let (_dir, repo) = temp_repo();
    let result = repo.diff_paths(
        "0000000000000000000000000000000000000000",
        &repo.head_oid().unwrap().to_string(),
    );
    assert!(result.is_err());
}

#[test]
fn merge_conflicts_populate_hlc() {
    let (_da, repo_a, _db, repo_b, _bare) = setup_two_repos();

    let hlc_a = crate::hlc::Hlc {
        wall_ms: 5000,
        counter: 0,
        node: "aaa".into(),
    };
    let msg_a = crate::hlc::append_hlc_trailer("A edits", &hlc_a);
    repo_a
        .commit_file("ddb/note.md", "version A", &msg_a)
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    let hlc_b = crate::hlc::Hlc {
        wall_ms: 6000,
        counter: 0,
        node: "bbb".into(),
    };
    let msg_b = crate::hlc::append_hlc_trailer("B edits", &hlc_b);
    repo_b
        .commit_file("ddb/note.md", "version B", &msg_b)
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    match result {
        MergeResult::Conflicts(conflicts, _) => {
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].ours_hlc.as_ref().unwrap().wall_ms, 6000);
            assert_eq!(conflicts[0].theirs_hlc.as_ref().unwrap().wall_ms, 5000);
        }
        other => panic!("expected Conflicts, got {:?}", other),
    }
}

#[test]
fn find_hlc_for_path_returns_hlc_when_trailer_present() {
    let (_dir, repo) = temp_repo();
    let hlc = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 1,
        node: "abc".into(),
    };
    let msg = crate::hlc::append_hlc_trailer("add doogat", &hlc);
    repo.commit_file("ddb/20260226120000.md", "---\ntitle: test\n---\n", &msg)
        .unwrap();
    let head = repo.head_commit().unwrap();
    let result = repo.find_hlc_for_path(&head, "ddb/20260226120000.md");
    assert!(result.is_some());
    assert_eq!(result.unwrap().wall_ms, 1000);
}

#[test]
fn find_hlc_for_path_returns_none_without_trailer() {
    let (_dir, repo) = temp_repo();
    repo.commit_file(
        "ddb/20260226120000.md",
        "---\ntitle: test\n---\n",
        "add doogat",
    )
    .unwrap();
    let head = repo.head_commit().unwrap();
    let result = repo.find_hlc_for_path(&head, "ddb/20260226120000.md");
    assert!(result.is_none());
}

#[test]
fn find_hlc_for_path_returns_none_for_untouched_path() {
    let (_dir, repo) = temp_repo();
    let hlc = crate::hlc::Hlc {
        wall_ms: 2000,
        counter: 0,
        node: "xyz".into(),
    };
    let msg = crate::hlc::append_hlc_trailer("add doogat", &hlc);
    repo.commit_file("ddb/20260226120000.md", "test", &msg)
        .unwrap();
    let head = repo.head_commit().unwrap();
    let result = repo.find_hlc_for_path(&head, "ddb/99990101000000.md");
    assert!(result.is_none());
}

#[test]
fn rename_file_moves_and_commits() {
    let (dir, repo) = temp_repo();
    repo.commit_file("ddb/20260301120000.md", "hello", "add")
        .unwrap();
    let hash = repo
        .rename_file(
            "ddb/20260301120000.md",
            "ddb/contact/20260301120000.md",
            "rename",
        )
        .unwrap();
    assert!(!hash.0.is_empty());
    assert!(!dir.path().join("ddb/20260301120000.md").exists());
    assert!(dir.path().join("ddb/contact/20260301120000.md").exists());
    let content =
        std::fs::read_to_string(dir.path().join("ddb/contact/20260301120000.md")).unwrap();
    assert_eq!(content, "hello");
}

#[test]
fn rename_file_errors_on_missing_source() {
    let (_dir, repo) = temp_repo();
    let err = repo
        .rename_file("ddb/nonexistent.md", "ddb/new.md", "rename")
        .unwrap_err();
    assert!(matches!(err, DoogatError::NotFound(_)));
}

#[test]
fn rename_file_errors_on_existing_target() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/a.md", "a", "add a").unwrap();
    repo.commit_file("ddb/b.md", "b", "add b").unwrap();
    let err = repo
        .rename_file("ddb/a.md", "ddb/b.md", "rename")
        .unwrap_err();
    assert!(matches!(err, DoogatError::InvalidPath(_)));
}

#[test]
fn rename_doogat_all_links_resolved() {
    let (dir, repo) = temp_repo();
    let index = crate::indexer::Index::open(&dir.path().join("index.db")).unwrap();

    // Create target doogat A
    let doogat_a = "---\nid: 20260301100000\ntitle: Target\n---\nBody\n";
    repo.commit_file("ddb/20260301100000.md", doogat_a, "add A")
        .unwrap();

    // Create doogat B linking to A via bare ID (matched by backlinking_doogat_paths)
    let doogat_b = "---\nid: 20260301110000\ntitle: Linker\n---\nSee [[20260301100000|Target]]\n";
    repo.commit_file("ddb/20260301110000.md", doogat_b, "add B")
        .unwrap();

    // Index both
    let parsed_a = crate::parser::parse(doogat_a, "ddb/20260301100000.md").unwrap();
    let parsed_b = crate::parser::parse(doogat_b, "ddb/20260301110000.md").unwrap();
    index.index_doogat(&parsed_a).unwrap();
    index.index_doogat(&parsed_b).unwrap();

    let report = rename_doogat(
        &repo,
        &index,
        "ddb/20260301100000.md",
        "ddb/contact/20260301100000.md",
    )
    .unwrap();

    assert_eq!(report.updated.len(), 1, "B's link should be rewritten");
    assert!(
        report.unresolvable.is_empty(),
        "expected no unresolvable, got: {:?}",
        report.unresolvable
    );
}

#[test]
fn rename_doogat_no_backlinks_means_empty_report() {
    let (dir, repo) = temp_repo();
    let index = crate::indexer::Index::open(&dir.path().join("index.db")).unwrap();

    // Create a lone doogat with no backlinks
    let doogat_a = "---\nid: 20260301100000\ntitle: Alone\n---\nBody\n";
    repo.commit_file("ddb/20260301100000.md", doogat_a, "add A")
        .unwrap();

    let parsed_a = crate::parser::parse(doogat_a, "ddb/20260301100000.md").unwrap();
    index.index_doogat(&parsed_a).unwrap();

    let report = rename_doogat(
        &repo,
        &index,
        "ddb/20260301100000.md",
        "ddb/contact/20260301100000.md",
    )
    .unwrap();

    assert!(report.updated.is_empty());
    assert!(report.unresolvable.is_empty());
}

#[test]
fn rename_doogat_detects_unresolvable_path_qualified_link() {
    let (dir, repo) = temp_repo();
    let index = crate::indexer::Index::open(&dir.path().join("index.db")).unwrap();

    // Create target doogat A
    let doogat_a = "---\nid: 20260301100000\ntitle: Target\n---\nBody\n";
    repo.commit_file("ddb/20260301100000.md", doogat_a, "add A")
        .unwrap();

    // Create doogat B with path-qualified wikilink (without .md).
    // backlinking_doogat_paths queries by exact target_path, which stores
    // "ddb/20260301100000" — this won't match old_path
    // "ddb/20260301100000.md" or old_id "20260301100000",
    // so the link is never found for rewriting.
    let doogat_b = "---\nid: 20260301110000\ntitle: PathLinker\n---\n\
            See [[ddb/20260301100000|Target]]\n";
    repo.commit_file("ddb/20260301110000.md", doogat_b, "add B")
        .unwrap();

    let parsed_a = crate::parser::parse(doogat_a, "ddb/20260301100000.md").unwrap();
    let parsed_b = crate::parser::parse(doogat_b, "ddb/20260301110000.md").unwrap();
    index.index_doogat(&parsed_a).unwrap();
    index.index_doogat(&parsed_b).unwrap();

    let report = rename_doogat(
        &repo,
        &index,
        "ddb/20260301100000.md",
        "ddb/contact/20260301100000.md",
    )
    .unwrap();

    // B's link was not rewritten (backlinking_doogat_paths missed it),
    // so step 5 should detect it as unresolvable
    assert!(
        !report.unresolvable.is_empty(),
        "expected unresolvable reference from B's path-qualified link"
    );
    assert_eq!(report.unresolvable[0], "ddb/20260301110000.md");
}

#[test]
fn doogat_path_flat() {
    assert_eq!(
        super::doogat_path("20260315120000", Some("contact"), false),
        "ddb/20260315120000.md"
    );
}

#[test]
fn doogat_path_folder() {
    assert_eq!(
        super::doogat_path("20260315120000", Some("contact"), true),
        "ddb/contact/20260315120000.md"
    );
}

#[test]
fn doogat_path_no_type() {
    assert_eq!(
        super::doogat_path("20260315120000", None, true),
        "ddb/20260315120000.md"
    );
}

/// Regression guard for the cross-process write lock (PRD 00162): N threads,
/// each with its OWN `GitRepo` on ONE shared repo, each committing a distinct
/// doogat, must ALL land in HEAD. Without the lock, two writers can resolve the
/// same stale parent and force-update the ref, silently dropping a peer's file.
#[test]
fn concurrent_commits_land_all_writes_in_head() {
    use std::sync::Arc;

    let (dir, _repo) = temp_repo();
    let root = Arc::new(dir.path().to_path_buf());
    let n = 8;

    let mut handles = Vec::new();
    for i in 0..n {
        let root = Arc::clone(&root);
        handles.push(std::thread::spawn(move || {
            // Each thread opens its own GitRepo (own git2::Repository, own lock
            // fd) on the shared repo — the realistic multi-writer shape.
            let repo = GitRepo::open(root.as_path()).unwrap();
            let id = format!("2026010100{i:04}"); // distinct 14-digit id
            let rel = format!("ddb/{id}.md");
            repo.commit_file(&rel, &format!("content {i}"), &format!("add {id}"))
                .unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // Every distinct doogat must be present in HEAD — none lost to a race.
    let repo = GitRepo::open(root.as_path()).unwrap();
    let doogats = repo.list_doogats().unwrap();
    for i in 0..n {
        let rel = format!("ddb/2026010100{i:04}.md");
        assert!(
            doogats.iter().any(|p| p == &rel),
            "lost update: {rel} missing from HEAD; present = {doogats:?}"
        );
    }
}

#[test]
fn delete_file_builds_tree_from_fresh_index() {
    let (dir, repo) = temp_repo();
    repo.commit_file("ddb/g.md", "g content", "add g").unwrap();

    let external = GitRepo::open(dir.path()).unwrap();
    external
        .commit_file("ddb/f.md", "f content", "add f")
        .unwrap();

    repo.delete_file("ddb/g.md", "delete g").unwrap();

    let content = repo.read_file("ddb/f.md").unwrap();
    assert_eq!(content, "f content");
    assert!(repo.read_file("ddb/g.md").is_err());
}

#[test]
fn delete_files_builds_tree_from_fresh_index() {
    let (dir, repo) = temp_repo();
    repo.commit_files(
        &[("ddb/g1.md", "g1 content"), ("ddb/g2.md", "g2 content")],
        "add g1 g2",
    )
    .unwrap();

    let external = GitRepo::open(dir.path()).unwrap();
    external
        .commit_file("ddb/f.md", "f content", "add f")
        .unwrap();

    repo.delete_files(&["ddb/g1.md", "ddb/g2.md"], "delete g1 g2")
        .unwrap();

    let content = repo.read_file("ddb/f.md").unwrap();
    assert_eq!(content, "f content");
    assert!(repo.read_file("ddb/g1.md").is_err());
    assert!(repo.read_file("ddb/g2.md").is_err());
}

/// Merge write path (PRD 00163): `merge_remote` must run its critical section
/// under the repo write lock, and a writer that committed while holding that
/// lock must survive a subsequent merge — its commit stays an ancestor of the
/// post-merge HEAD and its content stays in the tree.
///
/// Fails against the current UNLOCKED merge code: while the helper thread holds
/// the raw write guard, an unlocked `merge_remote` runs and returns immediately
/// (before the guard is released), so `released` is still false. Passes once the
/// merge path holds the lock, because the merge then blocks on the guard until
/// it is dropped.
#[test]
fn merge_blocks_while_write_lock_held() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::Arc;

    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_two_repos();

    // Give origin a change to merge and fetch it into repo_b, but do NOT merge.
    repo_a
        .commit_file("ddb/x.md", "from a", "a change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let dir_b_path = dir_b.path().to_path_buf();
    let released = Arc::new(AtomicBool::new(false));
    let released_thread = Arc::clone(&released);
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    let handle = std::thread::spawn(move || {
        // Hold the repo's raw OS write guard across the whole critical window.
        let guard = super::write_lock::acquire(&dir_b_path, Duration::from_secs(10)).unwrap();

        // Commit a NEW distinct file with raw git2 on refs/heads/master. Raw
        // git2 on purpose: GitRepo's own write methods would block on the very
        // lock this thread already holds.
        let raw = Repository::open(&dir_b_path).unwrap();
        std::fs::write(dir_b_path.join("ddb/y.md"), "from helper").unwrap();
        let mut index = raw.index().unwrap();
        index.add_path(Path::new("ddb/y.md")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = raw.find_tree(tree_oid).unwrap();
        let sig = Signature::now("helper", "helper@local").unwrap();
        let parent = raw.head().unwrap().peel_to_commit().unwrap();
        let helper_oid = raw
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "helper commit under lock",
                &tree,
                &[&parent],
            )
            .unwrap();

        // Signal: guard is held and the locked writer's commit has landed.
        tx.send(helper_oid.to_string()).unwrap();

        // Hold the guard for a bounded window well under the 10s timeout. An
        // unlocked merge returns inside this window; a locked one cannot.
        std::thread::sleep(Duration::from_millis(500));
        released_thread.store(true, SeqCst);
        drop(guard);
    });

    // Wait until the guard is held and the helper commit exists, then merge.
    let helper_oid = rx.recv().unwrap();
    let result = repo_b.merge_remote("origin", "master").unwrap();

    // Discriminating assertion: the merge must not have returned until the
    // guard was released. On unlocked code it returns while `released` is false.
    assert!(
        released.load(SeqCst),
        "merge_remote returned while the write lock was held — it ran unlocked"
    );
    handle.join().unwrap();

    // Correctness pin: the racing merge neither orphaned the locked writer's
    // commit nor dropped its content. This is a fixed-code correctness pin, not
    // a fail-first discriminator — the race has no reliable injection point on
    // old code; the ordering flag above is the discriminator.
    match result {
        MergeResult::Clean(_) => {}
        other => panic!("expected a clean merge commit, got {other:?}"),
    }
    let head = repo_b.head_oid().unwrap();
    let head_oid = Oid::from_str(&head.0).unwrap();
    let helper = Oid::from_str(&helper_oid).unwrap();
    assert!(
        repo_b.repo.graph_descendant_of(head_oid, helper).unwrap(),
        "helper's locked commit is not an ancestor of post-merge HEAD (orphaned)"
    );
    assert_eq!(
        repo_b.read_file("ddb/y.md").unwrap(),
        "from helper",
        "helper's committed file was dropped from the merged tree"
    );
}

/// Behavior-preservation guard for the merge-tree staging base: a conflicted
/// merge resolved via `commit_merge` must yield the same observable tree
/// regardless of the internal staging base — a theirs-only file keeps theirs'
/// content and a both-sides-changed file carries the resolved content. Passes
/// on both old and new code (guards against accidental drift, not fail-first).
#[test]
fn conflicted_merge_arm_behavior_unchanged() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    // Shared base file, brought up to date on repo_b via fast-forward.
    repo_a
        .commit_file("ddb/x.md", "base", "base x")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    // Theirs: add a theirs-only file AND edit the shared file.
    repo_a
        .commit_files(
            &[("ddb/t.md", "theirs t"), ("ddb/x.md", "theirs x")],
            "theirs change",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: a conflicting local edit to the shared file.
    repo_b
        .commit_file("ddb/x.md", "ours x", "ours change")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    repo_b
        .commit_merge(
            &[("ddb/x.md", "resolved x")],
            &[],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    // Observable arm outcomes that must not drift under a staging-base change:
    assert_eq!(repo_b.read_file("ddb/t.md").unwrap(), "theirs t");
    assert_eq!(repo_b.read_file("ddb/x.md").unwrap(), "resolved x");
}

/// Regression guard for `commit_merge`'s OWN write lock (PRD 00163): it has a
/// `with_write_lock` wrap separate from `merge_remote`'s, and
/// `merge_blocks_while_write_lock_held` above only exercises `merge_remote`.
/// Nothing pins `commit_merge` itself, so removing its wrap passes every other
/// test.
///
/// Fails against an unlocked `commit_merge`: while the helper thread holds the
/// raw write guard, an unlocked `commit_merge` runs and returns immediately
/// (before the guard is released), so `released` is still false. Passes once
/// `commit_merge` holds the lock, because it then blocks on the guard until it
/// is dropped.
#[test]
fn commit_merge_blocks_while_write_lock_held() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::Arc;

    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_two_repos();

    // Reach a Conflicts state so commit_merge is the method under test.
    repo_a.commit_file("ddb/x.md", "base", "base x").unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    repo_a
        .commit_files(
            &[("ddb/t.md", "theirs t"), ("ddb/x.md", "theirs x")],
            "theirs change",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/x.md", "ours x", "ours change")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let dir_b_path = dir_b.path().to_path_buf();
    let released = Arc::new(AtomicBool::new(false));
    let released_thread = Arc::clone(&released);
    let (tx, rx) = std::sync::mpsc::channel::<()>();

    let handle = std::thread::spawn(move || {
        // Hold the repo's raw OS write guard across the whole critical window.
        let guard = super::write_lock::acquire(&dir_b_path, Duration::from_secs(10)).unwrap();
        tx.send(()).unwrap();

        // Hold the guard for a bounded window well under the 10s timeout. An
        // unlocked commit_merge returns inside this window; a locked one cannot.
        std::thread::sleep(Duration::from_millis(500));
        released_thread.store(true, SeqCst);
        drop(guard);
    });

    // Wait until the guard is held, then call commit_merge.
    rx.recv().unwrap();
    repo_b
        .commit_merge(
            &[("ddb/x.md", "resolved x")],
            &[],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    // Discriminating assertion: commit_merge must not have returned until the
    // guard was released. On unlocked code it returns while `released` is false.
    assert!(
        released.load(SeqCst),
        "commit_merge returned while the write lock was held — it ran unlocked"
    );
    handle.join().unwrap();

    // Light correctness pins.
    assert_eq!(repo_b.read_file("ddb/x.md").unwrap(), "resolved x");
    assert_eq!(repo_b.read_file("ddb/t.md").unwrap(), "theirs t");
}

/// A conflicted merge must keep ours' non-conflicting edit to a file theirs
/// never touched. Fails if the merge tree reverts ours' edit back to base.
#[test]
fn conflicted_merge_keeps_ours_only_edit() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_files(&[("ddb/x.md", "base-x"), ("ddb/z.md", "base-z")], "base")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    // Theirs: edit only the shared file.
    repo_a
        .commit_file("ddb/x.md", "theirs x", "theirs change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: edit z.md AND conflict on the shared file.
    repo_b.commit_file("ddb/z.md", "ours-z", "ours z").unwrap();
    repo_b
        .commit_file("ddb/x.md", "ours x", "ours x")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    repo_b
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid)
        .unwrap();

    // ours' non-conflicting edit survives, AND the conflict carries the passed
    // resolution (not a re-picked side) — the latter rejects an impl that
    // ignores `files`.
    assert_eq!(repo_b.read_file("ddb/z.md").unwrap(), "ours-z");
    assert_eq!(repo_b.read_file("ddb/x.md").unwrap(), "resolved x");
}

/// A conflicted merge must keep ours' clean deletion of a file theirs never
/// touched. Fails if the merge tree resurrects the deleted file.
#[test]
fn conflicted_merge_keeps_ours_clean_delete() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_files(&[("ddb/x.md", "base-x"), ("ddb/d.md", "base-d")], "base")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    repo_a
        .commit_file("ddb/x.md", "theirs x", "theirs change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: delete d.md AND conflict on the shared file.
    repo_b.delete_file("ddb/d.md", "ours delete d").unwrap();
    repo_b
        .commit_file("ddb/x.md", "ours x", "ours x")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    repo_b
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid)
        .unwrap();

    assert!(repo_b.read_file("ddb/d.md").is_err());
}

/// A conflicted merge must honor theirs' deletion of a file ours never
/// touched. Fails if the merge tree resurrects the file ours still carries.
#[test]
fn conflicted_merge_drops_theirs_deletion() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_files(&[("ddb/x.md", "base-x"), ("ddb/y.md", "base-y")], "base")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    // Theirs: delete y.md AND edit the shared file.
    repo_a.delete_file("ddb/y.md", "theirs delete y").unwrap();
    repo_a
        .commit_file("ddb/x.md", "theirs x", "theirs change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: conflict on the shared file only (y untouched).
    repo_b
        .commit_file("ddb/x.md", "ours x", "ours x")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    repo_b
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid)
        .unwrap();

    assert!(repo_b.read_file("ddb/y.md").is_err());
}

/// A conflicted merge must keep a brand-new doogat ours created that never
/// existed in base or theirs. Fails if the merge tree drops ours' creation.
#[test]
fn conflicted_merge_keeps_ours_created_doogat() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/x.md", "base-x", "base")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    repo_a
        .commit_file("ddb/x.md", "theirs x", "theirs change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: create a new doogat AND conflict on the shared file.
    repo_b
        .commit_file("ddb/n.md", "ours-n", "ours new n")
        .unwrap();
    repo_b
        .commit_file("ddb/x.md", "ours x", "ours x")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    repo_b
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid)
        .unwrap();

    assert_eq!(repo_b.read_file("ddb/n.md").unwrap(), "ours-n");
}

/// A conflicted merge must auto-merge non-overlapping line edits from both
/// sides on a file that is NOT passed to commit_merge. Fails if either side's
/// line edit is dropped.
#[test]
fn conflicted_merge_line_merges_both_edits() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_files(
            &[("ddb/x.md", "base-x"), ("ddb/b4.md", "line1\nline2\nline3\n")],
            "base",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    // Theirs: edit ONLY line 3 of b4 AND conflict on the shared file.
    repo_a
        .commit_files(
            &[
                ("ddb/b4.md", "line1\nline2\ntheirs-line3\n"),
                ("ddb/x.md", "theirs x"),
            ],
            "theirs change",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: edit ONLY line 1 of b4 AND conflict on the shared file.
    repo_b
        .commit_files(
            &[
                ("ddb/b4.md", "ours-line1\nline2\nline3\n"),
                ("ddb/x.md", "ours x"),
            ],
            "ours change",
        )
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    // b4.md is auto-merged by git; only x.md is passed as resolved.
    repo_b
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid)
        .unwrap();

    let b4 = repo_b.read_file("ddb/b4.md").unwrap();
    assert!(b4.contains("ours-line1"), "ours' line-1 edit was dropped: {b4:?}");
    assert!(
        b4.contains("theirs-line3"),
        "theirs' line-3 edit was dropped: {b4:?}"
    );
}

/// A conflicted merge must carry theirs' rename as one doogat at the new path
/// and none at the old. Fails if the rename is duplicated or reverted.
#[test]
fn conflicted_merge_theirs_rename_single_path() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_files(&[("ddb/x.md", "base-x"), ("ddb/z.md", "base-z")], "base")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    // Theirs: rename z.md -> z2.md AND edit the shared file.
    repo_a
        .rename_file("ddb/z.md", "ddb/z2.md", "theirs rename z")
        .unwrap();
    repo_a
        .commit_file("ddb/x.md", "theirs x", "theirs change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/x.md", "ours x", "ours x")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    repo_b
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid)
        .unwrap();

    assert!(repo_b.read_file("ddb/z2.md").is_ok());
    assert!(repo_b.read_file("ddb/z.md").is_err());
}

/// A conflicted merge must carry ours' rename as one doogat at the new path
/// and none at the old. Fails if the rename is duplicated or reverted.
#[test]
fn conflicted_merge_ours_rename_single_path() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_files(&[("ddb/x.md", "base-x"), ("ddb/w.md", "base-w")], "base")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    repo_a
        .commit_file("ddb/x.md", "theirs x", "theirs change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: rename w.md -> w2.md AND conflict on the shared file.
    repo_b
        .rename_file("ddb/w.md", "ddb/w2.md", "ours rename w")
        .unwrap();
    repo_b
        .commit_file("ddb/x.md", "ours x", "ours x")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    repo_b
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid)
        .unwrap();

    assert!(repo_b.read_file("ddb/w2.md").is_ok());
    assert!(repo_b.read_file("ddb/w.md").is_err());
}

/// If ours' HEAD moves after the conflict set was computed, commit_merge must
/// fail loud with a Conflict error and leave HEAD untouched (no merge commit).
/// Fails if commit_merge commits against a stale, diverged base.
#[test]
fn conflicted_merge_diverged_head_fails_loud() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/x.md", "base-x", "base")
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    repo_a
        .commit_file("ddb/x.md", "theirs x", "theirs change")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/x.md", "ours x", "ours x")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    // Move ours' HEAD so the x.md conflict is resolved away (ours now matches
    // theirs), changing the merge's conflict set. The previously-computed
    // resolution is now stale.
    repo_b
        .commit_file("ddb/x.md", "theirs x", "ours converges to theirs")
        .unwrap();
    let head_before = repo_b.head_oid().unwrap();

    let merge_result =
        repo_b.commit_merge(&[("ddb/x.md", "resolved x")], &[], "merge", &theirs_oid);
    assert!(
        matches!(merge_result, Err(DoogatError::Conflict(_))),
        "expected Conflict error on diverged HEAD, got {merge_result:?}"
    );

    // HEAD is unchanged: no merge commit was created.
    assert_eq!(repo_b.head_oid().unwrap().0, head_before.0);
    assert_ne!(repo_b.read_file("ddb/x.md").unwrap(), "resolved x");
}
