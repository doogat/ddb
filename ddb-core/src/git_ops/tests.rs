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
