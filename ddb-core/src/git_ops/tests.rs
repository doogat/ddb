use super::*;
use tempfile::TempDir;

fn temp_repo() -> (TempDir, GitRepo) {
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    (dir, repo)
}

/// Read and parse the machine-local node HLC persisted at `<git_dir>/ddb-hlc`.
fn node_hlc(repo: &GitRepo) -> crate::hlc::Hlc {
    let raw = std::fs::read_to_string(repo.repo.path().join("ddb-hlc")).unwrap();
    crate::hlc::Hlc::parse(raw.trim()).unwrap()
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
    let (dir_a, repo_a, dir_b, repo_b, _bare) = setup_two_repos();

    // Control each repo's clock by PRE-SEEDING its machine-local `.git/ddb-hlc`
    // with a far-future value before the write; the write API's auto-stamp then
    // carries that `wall_ms` through (a below-wall-now seed would be superseded
    // by wall-now). Commit with a PLAIN message — the API stamps its own trailer,
    // so a manual `append_hlc_trailer` would be shadowed by it. theirs (repo_a)
    // is seeded strictly LOWER than ours (repo_b) so the ordering is observable.
    let theirs_seed = crate::hlc::Hlc {
        wall_ms: u64::MAX / 2,
        counter: 0,
        node: "theirsss".into(),
    };
    std::fs::write(dir_a.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    let ours_seed = crate::hlc::Hlc {
        wall_ms: u64::MAX / 2 + 1_000_000,
        counter: 0,
        node: "oursssss".into(),
    };
    std::fs::write(dir_b.path().join(".git/ddb-hlc"), ours_seed.to_string()).unwrap();
    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    match result {
        MergeResult::Conflicts(conflicts, _) => {
            assert_eq!(conflicts.len(), 1);
            let ours_hlc = conflicts[0].ours_hlc.as_ref().unwrap();
            let theirs_hlc = conflicts[0].theirs_hlc.as_ref().unwrap();
            // Each side's conflict HLC reflects its OWN far-future seed ...
            assert_eq!(ours_hlc.wall_ms, ours_seed.wall_ms);
            assert_eq!(theirs_hlc.wall_ms, theirs_seed.wall_ms);
            // ... and ours (seeded higher) orders after theirs.
            assert!(
                ours_hlc.wall_ms > theirs_hlc.wall_ms,
                "ours' seeded HLC must order after theirs': {} !> {}",
                ours_hlc.wall_ms,
                theirs_hlc.wall_ms
            );
        }
        other => panic!("expected Conflicts, got {:?}", other),
    }
}

#[test]
fn find_hlc_for_path_returns_hlc_when_trailer_present() {
    let (dir, repo) = temp_repo();
    let hlc = crate::hlc::Hlc {
        wall_ms: 1000,
        counter: 1,
        node: "abc".into(),
    };
    let msg = crate::hlc::append_hlc_trailer("add doogat", &hlc);

    // Raw commit so exactly ONE known trailer is present. Committing through the
    // public write API would append a second, auto-stamped trailer that shadows
    // this injected one, making the exact-value assertion meaningless.
    std::fs::write(
        dir.path().join("ddb/20260226120000.md"),
        "---\ntitle: test\n---\n",
    )
    .unwrap();
    let raw = &repo.repo;
    let sig = Signature::now("ddb", "ddb@local").unwrap();
    let parent = raw.head().unwrap().peel_to_commit().unwrap();
    let mut index = raw.index().unwrap();
    index.read_tree(&parent.tree().unwrap()).unwrap();
    index.add_path(Path::new("ddb/20260226120000.md")).unwrap();
    index.write().unwrap();
    let tree = raw.find_tree(index.write_tree().unwrap()).unwrap();
    raw.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&parent])
        .unwrap();

    let head = repo.head_commit().unwrap();
    let result = repo.find_hlc_for_path(&head, "ddb/20260226120000.md");
    assert!(result.is_some());
    let found = result.unwrap();
    assert_eq!(found.wall_ms, 1000);
    assert_eq!(found.counter, 1);
    assert_eq!(found.node, "abc");
}

#[test]
fn find_hlc_for_path_returns_none_without_trailer() {
    let (dir, repo) = temp_repo();

    // The public write API now always stamps an HLC trailer, so a trailer-less
    // commit (legacy history) must be built raw to keep this reader case honest.
    std::fs::write(
        dir.path().join("ddb/20260226120000.md"),
        "---\ntitle: test\n---\n",
    )
    .unwrap();
    let raw = &repo.repo;
    let sig = Signature::now("ddb", "ddb@local").unwrap();
    let parent = raw.head().unwrap().peel_to_commit().unwrap();
    let mut index = raw.index().unwrap();
    index.read_tree(&parent.tree().unwrap()).unwrap();
    index.add_path(Path::new("ddb/20260226120000.md")).unwrap();
    index.write().unwrap();
    let tree = raw.find_tree(index.write_tree().unwrap()).unwrap();
    raw.commit(Some("HEAD"), &sig, &sig, "add doogat", &tree, &[&parent])
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
fn find_hlc_for_path_reaches_beyond_old_revwalk_cap() {
    // PRD 00166 scope addition: the revwalk depth cap must not silently drop an
    // HLC that lives deeper than the OLD cap (100). A doogat touched once, then
    // buried under 120 unrelated commits (>100 old cap, <1000 new cap), must
    // still resolve its HLC. This FAILS if MAX_REVWALK_DEPTH regresses to 100
    // (the walk warns + returns None at depth 100, before reaching the touch at
    // depth 120) and passes at 1000.
    let (dir, repo) = temp_repo();
    let hlc = crate::hlc::Hlc {
        wall_ms: 4242,
        counter: 7,
        node: "deep".into(),
    };
    let msg = crate::hlc::append_hlc_trailer("add deep doogat", &hlc);
    let raw = &repo.repo;
    let sig = Signature::now("ddb", "ddb@local").unwrap();

    // Bottom commit: touches the target path, carries the known HLC trailer.
    // Raw commit so exactly one known trailer is present (the public write API
    // would auto-stamp a second, shadowing trailer).
    std::fs::write(
        dir.path().join("ddb/20260226120000.md"),
        "---\ntitle: deep\n---\n",
    )
    .unwrap();
    let parent = raw.head().unwrap().peel_to_commit().unwrap();
    let mut index = raw.index().unwrap();
    index.read_tree(&parent.tree().unwrap()).unwrap();
    index.add_path(Path::new("ddb/20260226120000.md")).unwrap();
    index.write().unwrap();
    let tree = raw.find_tree(index.write_tree().unwrap()).unwrap();
    raw.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&parent])
        .unwrap();

    // Bury it under 120 commits that touch only an UNRELATED path.
    for i in 0..120 {
        std::fs::write(dir.path().join("ddb/_filler.md"), format!("filler {i}\n")).unwrap();
        let parent = raw.head().unwrap().peel_to_commit().unwrap();
        let mut index = raw.index().unwrap();
        index.read_tree(&parent.tree().unwrap()).unwrap();
        index.add_path(Path::new("ddb/_filler.md")).unwrap();
        index.write().unwrap();
        let tree = raw.find_tree(index.write_tree().unwrap()).unwrap();
        raw.commit(Some("HEAD"), &sig, &sig, "filler", &tree, &[&parent])
            .unwrap();
    }

    let head = repo.head_commit().unwrap();
    let result = repo.find_hlc_for_path(&head, "ddb/20260226120000.md");
    assert!(
        result.is_some(),
        "HLC buried 120 commits deep must resolve after the cap was raised to 1000 \
         (regresses to None if MAX_REVWALK_DEPTH drops back to 100)"
    );
    let found = result.unwrap();
    assert_eq!(found.wall_ms, 4242);
    assert_eq!(found.counter, 7);
    assert_eq!(found.node, "deep");
}

/// Every non-merge write path (create, update, rename, delete, batch) must stamp
/// a parseable `HLC:` trailer on its commit AND advance-and-persist the
/// machine-local node HLC. After each write the commit that touched the path
/// carries an HLC (delete and rename included), and the persisted `<git_dir>/ddb-hlc`
/// strictly increases across the whole sequence. A wrong impl that stamps one
/// hardcoded trailer, ticks only in memory without persisting, or skips the
/// delete/rename paths must FAIL this.
#[test]
fn every_non_merge_write_stamps_and_advances_hlc() {
    let (_dir, repo) = temp_repo();

    // The clock is persisted at init, before any write.
    assert!(
        repo.repo.path().join("ddb-hlc").exists(),
        "ddb-hlc must be persisted immediately after init"
    );
    let mut prev = node_hlc(&repo);

    // create
    repo.commit_file("ddb/20260401120000.md", "---\ntitle: a\n---\n", "create a")
        .unwrap();
    let head = repo.head_commit().unwrap();
    assert!(
        repo.find_hlc_for_path(&head, "ddb/20260401120000.md")
            .is_some(),
        "create commit must carry an HLC trailer"
    );
    let cur = node_hlc(&repo);
    assert!(cur > prev, "create must advance the persisted node HLC: {cur} !> {prev}");
    assert_eq!(
        repo.find_hlc_for_path(&head, "ddb/20260401120000.md")
            .unwrap(),
        node_hlc(&repo),
        "create commit's stamped HLC trailer must equal the persisted node clock"
    );
    // The stamped HLC must carry REAL wall-clock time (the clock is load-bearing,
    // not a fabricated constant). Rejects a fake clock frozen at epoch+1s.
    assert!(
        cur.wall_ms > 1_600_000_000_000 && cur.wall_ms < 10_000_000_000_000,
        "stamped HLC must carry real wall-clock time, got {}",
        cur.wall_ms
    );
    prev = cur;

    // update (second write to the same path)
    repo.commit_file("ddb/20260401120000.md", "---\ntitle: a2\n---\n", "update a")
        .unwrap();
    let head = repo.head_commit().unwrap();
    assert!(
        repo.find_hlc_for_path(&head, "ddb/20260401120000.md")
            .is_some(),
        "update commit must carry an HLC trailer"
    );
    let cur = node_hlc(&repo);
    assert!(cur > prev, "update must advance the persisted node HLC: {cur} !> {prev}");
    assert_eq!(
        repo.find_hlc_for_path(&head, "ddb/20260401120000.md")
            .unwrap(),
        node_hlc(&repo),
        "update commit's stamped HLC trailer must equal the persisted node clock"
    );
    prev = cur;

    // rename (old -> new); the delete step below removes the renamed file
    repo.rename_file(
        "ddb/20260401120000.md",
        "ddb/contact/20260401120000.md",
        "rename a",
    )
    .unwrap();
    let head = repo.head_commit().unwrap();
    assert!(
        repo.find_hlc_for_path(&head, "ddb/contact/20260401120000.md")
            .is_some(),
        "rename commit must carry an HLC trailer on the new path"
    );
    let cur = node_hlc(&repo);
    assert!(cur > prev, "rename must advance the persisted node HLC: {cur} !> {prev}");
    assert_eq!(
        repo.find_hlc_for_path(&head, "ddb/contact/20260401120000.md")
            .unwrap(),
        node_hlc(&repo),
        "rename commit's stamped HLC trailer must equal the persisted node clock"
    );
    prev = cur;

    // delete (removes the renamed file)
    repo.delete_file("ddb/contact/20260401120000.md", "delete a")
        .unwrap();
    let head = repo.head_commit().unwrap();
    assert!(
        repo.find_hlc_for_path(&head, "ddb/contact/20260401120000.md")
            .is_some(),
        "delete commit must carry an HLC trailer on the deleted path"
    );
    let cur = node_hlc(&repo);
    assert!(cur > prev, "delete must advance the persisted node HLC: {cur} !> {prev}");
    assert_eq!(
        repo.find_hlc_for_path(&head, "ddb/contact/20260401120000.md")
            .unwrap(),
        node_hlc(&repo),
        "delete commit's stamped HLC trailer must equal the persisted node clock"
    );
    prev = cur;

    // batch write
    repo.commit_batch(&[("ddb/20260401140000.md", "content b")], &[], "batch b")
        .unwrap();
    let head = repo.head_commit().unwrap();
    assert!(
        repo.find_hlc_for_path(&head, "ddb/20260401140000.md")
            .is_some(),
        "batch commit must carry an HLC trailer"
    );
    let cur = node_hlc(&repo);
    assert!(cur > prev, "batch must advance the persisted node HLC: {cur} !> {prev}");
    assert_eq!(
        repo.find_hlc_for_path(&head, "ddb/20260401140000.md")
            .unwrap(),
        node_hlc(&repo),
        "batch commit's stamped HLC trailer must equal the persisted node clock"
    );
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
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid)
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
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid)
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
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid)
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
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid)
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
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid)
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
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid)
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
        .commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid)
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
        repo_b.commit_merge(&[("ddb/x.md", "resolved x")], &[], &[], "merge", &theirs_oid);
    assert!(
        matches!(merge_result, Err(DoogatError::Conflict(_))),
        "expected Conflict error on diverged HEAD, got {merge_result:?}"
    );

    // HEAD is unchanged: no merge commit was created.
    assert_eq!(repo_b.head_oid().unwrap().0, head_before.0);
    assert_ne!(repo_b.read_file("ddb/x.md").unwrap(), "resolved x");
}

/// When commit_merge aborts on a divergence, it must NOT have written the binary
/// winner's bytes to the worktree: the winner is overlaid by OID into the
/// in-memory tree and materialized only by the post-commit checkout. Fails if a
/// pre-write leaks stale winner bytes onto disk before the abort.
#[test]
fn conflicted_merge_abort_leaves_worktree_clean() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_files(
            &[("ddb/x.md", "base-x"), ("reference/foo/data.bin", "base-bin")],
            "base",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();
    repo_b.fetch("origin", "master").unwrap();
    let ff = repo_b.merge_remote("origin", "master").unwrap();
    assert!(matches!(ff, MergeResult::FastForward(_)));

    // Theirs: edit the shared text file AND the binary reference.
    repo_a
        .commit_files(
            &[
                ("ddb/x.md", "theirs x"),
                ("reference/foo/data.bin", "theirs-bin"),
            ],
            "theirs change",
        )
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours: conflict on BOTH the shared text file and the binary reference.
    repo_b
        .commit_files(
            &[
                ("ddb/x.md", "ours x"),
                ("reference/foo/data.bin", "ours-bin"),
            ],
            "ours change",
        )
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let (theirs_oid, winner_oid) = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            let bin = conflicts
                .iter()
                .find(|c| c.path == "reference/foo/data.bin")
                .expect("binary reference conflict");
            let winner = bin
                .theirs_blob_oid
                .clone()
                .expect("theirs blob OID for the binary conflict");
            assert!(conflicts.iter().any(|c| c.path == "ddb/x.md"));
            (theirs_oid, winner)
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    // Move ours' HEAD so the x.md conflict resolves away (ours converges to
    // theirs), shrinking the merge's conflict set to just the binary — which no
    // longer matches the resolved set {x.md, data.bin}. This forces the abort
    // while leaving the worktree binary at ours' pre-merge bytes.
    repo_b
        .commit_file("ddb/x.md", "theirs x", "ours converges to theirs")
        .unwrap();
    let head_before = repo_b.head_oid().unwrap();

    let bin_path = dir_b.path().join("reference/foo/data.bin");
    assert_eq!(std::fs::read(&bin_path).unwrap(), b"ours-bin");

    let merge_result = repo_b.commit_merge(
        &[("ddb/x.md", "resolved x")],
        &[("reference/foo/data.bin", winner_oid.as_str())],
        &[],
        "merge",
        &theirs_oid,
    );
    assert!(
        matches!(merge_result, Err(DoogatError::Conflict(_))),
        "expected Conflict error on diverged HEAD, got {merge_result:?}"
    );

    // The abort left the worktree binary untouched: still ours' bytes, NOT the
    // winner ("theirs-bin"). A pre-write would have stranded the winner here.
    assert_eq!(std::fs::read(&bin_path).unwrap(), b"ours-bin");
    assert_eq!(repo_b.head_oid().unwrap().0, head_before.0);
}

/// A clean (non-fast-forward) merge commit must carry a parseable HLC trailer
/// AND absorb the peer's clock: a peer skewed FAR into the future forces the
/// merge commit's own HLC `wall_ms >= that far-future seed`. Ours (repo_b) keeps
/// a normal wall-clock clock, so the ONLY way to reach the far-future band is by
/// absorbing theirs. A merge that stamps but does not absorb fails the `>=`
/// bound; a merge that absorbs but does not stamp fails `is_some` via `expect`.
#[test]
fn clean_merge_commit_carries_hlc_trailer_and_absorbs_theirs() {
    let (dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    // Peer (theirs, repo_a) is skewed far into the future; seed its `.git/ddb-hlc`
    // then commit a theirs-only file with a plain message so the auto-stamp
    // carries the far-future wall_ms.
    let far_future = u64::MAX / 2;
    let theirs_seed = crate::hlc::Hlc {
        wall_ms: far_future,
        counter: 0,
        node: "theirsss".into(),
    };
    std::fs::write(dir_a.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
    repo_a.commit_file("ddb/a.md", "from a", "add a").unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours diverges on a DIFFERENT file, so the merge is clean (non-FF).
    repo_b.commit_file("ddb/b.md", "from b", "add b").unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let merge_oid = match result {
        MergeResult::Clean(oid) => oid,
        other => panic!("expected a clean merge commit, got {other:?}"),
    };

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(
        merge_commit.id().to_string(),
        merge_oid.0,
        "HEAD must be the returned clean-merge commit"
    );
    let hlc = crate::hlc::extract_hlc(merge_commit.message().unwrap())
        .expect("clean merge commit must carry an HLC trailer");
    assert_eq!(
        hlc.wall_ms, far_future,
        "merge/next-write must absorb theirs' EXACT far-future wall_ms, not merely reach it: {} != {}",
        hlc.wall_ms, far_future
    );
}

/// A fast-forward merge produces NO merge commit, but must STILL absorb the
/// peer's clock: after fast-forwarding a peer skewed far into the future, ours'
/// NEXT ordinary write carries an HLC whose `wall_ms >= that far-future seed`.
/// Ours (repo_b) keeps a normal clock, so a FF that fails to absorb ticks the
/// next write from ~wall-now (far below the seed) and fails the `>=` bound.
#[test]
fn fast_forward_absorbs_theirs_hlc_into_next_write() {
    let (dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    // Peer (repo_a) skewed far into the future; ours (repo_b) stays at wall-clock.
    let far_future = u64::MAX / 2;
    let theirs_seed = crate::hlc::Hlc {
        wall_ms: far_future,
        counter: 0,
        node: "theirsss".into(),
    };
    std::fs::write(dir_a.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
    repo_a.commit_file("ddb/a.md", "from a", "add a").unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours has no diverging commit, so the merge fast-forwards (no merge commit).
    repo_b.fetch("origin", "master").unwrap();
    let result = repo_b.merge_remote("origin", "master").unwrap();
    assert!(
        matches!(result, MergeResult::FastForward(_)),
        "expected a fast-forward merge, got {result:?}"
    );

    // The FF created no merge commit, but must have absorbed theirs' clock: the
    // NEXT ordinary write advances past the far-future seed.
    repo_b
        .commit_file("ddb/c.md", "from b", "next write")
        .unwrap();
    let next = repo_b.head_commit().unwrap();
    let hlc = crate::hlc::extract_hlc(next.message().unwrap())
        .expect("ordinary write after fast-forward must carry an HLC trailer");
    assert_eq!(
        hlc.wall_ms, far_future,
        "merge/next-write must absorb theirs' EXACT far-future wall_ms, not merely reach it: {} != {}",
        hlc.wall_ms, far_future
    );
}

/// A conflict-resolution merge commit (produced by `commit_merge` after a
/// conflicted `merge_remote`) must carry a parseable HLC trailer AND absorb the
/// peer's clock: a peer skewed far into the future forces the resolution
/// commit's own HLC `wall_ms >= that far-future seed`. Ours (repo_b) keeps a
/// normal clock, so a resolution that stamps but does not absorb fails the `>=`
/// bound; one that absorbs but does not stamp fails `is_some` via `expect`.
#[test]
fn resolved_merge_commit_carries_hlc_trailer_and_absorbs_theirs() {
    let (dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    // Peer (theirs, repo_a) skewed far into the future; ours (repo_b) at wall-clock.
    let far_future = u64::MAX / 2;
    let theirs_seed = crate::hlc::Hlc {
        wall_ms: far_future,
        counter: 0,
        node: "theirsss".into(),
    };
    std::fs::write(dir_a.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours conflicts on the SAME file with a normal clock.
    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(
        merge_commit.id().to_string(),
        merge_oid.0,
        "HEAD must be the returned resolution merge commit"
    );
    let hlc = crate::hlc::extract_hlc(merge_commit.message().unwrap())
        .expect("resolution merge commit must carry an HLC trailer");
    assert_eq!(
        hlc.wall_ms, far_future,
        "merge/next-write must absorb theirs' EXACT far-future wall_ms, not merely reach it: {} != {}",
        hlc.wall_ms, far_future
    );
}

/// Ordinary-peer mirror of `clean_merge_commit_carries_hlc_trailer_and_absorbs_theirs`:
/// with a NON-skewed peer, the clean merge commit's HLC trailer must land in the
/// wall-clock band, NOT a far-future constant. Paired with its far-future sibling, no
/// hardcoded constant can satisfy both cases — the merge must genuinely fold in theirs'
/// HLC.
#[test]
fn clean_merge_ordinary_peer_stamps_wall_clock_band() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    // Peer (theirs, repo_a) is NOT seeded: it commits with its natural
    // current-wall-clock HLC.
    repo_a.commit_file("ddb/a.md", "from a", "add a").unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours diverges on a DIFFERENT file, so the merge is clean (non-FF).
    repo_b.commit_file("ddb/b.md", "from b", "add b").unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let merge_oid = match result {
        MergeResult::Clean(oid) => oid,
        other => panic!("expected a clean merge commit, got {other:?}"),
    };

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(
        merge_commit.id().to_string(),
        merge_oid.0,
        "HEAD must be the returned clean-merge commit"
    );
    let hlc = crate::hlc::extract_hlc(merge_commit.message().unwrap())
        .expect("clean merge commit must carry an HLC trailer");
    assert!(
        hlc.wall_ms > 1_600_000_000_000 && hlc.wall_ms < 10_000_000_000_000,
        "with an ordinary (non-skewed) peer the trailer must be in the wall-clock band, not a far-future constant: {}",
        hlc.wall_ms
    );
}

/// Ordinary-peer mirror of `fast_forward_absorbs_theirs_hlc_into_next_write`: with a
/// NON-skewed peer, the post-fast-forward next write's HLC trailer must land in the
/// wall-clock band, NOT a far-future constant.
#[test]
fn fast_forward_ordinary_peer_next_write_wall_clock_band() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    // Peer (repo_a) is NOT seeded: it commits at its natural wall clock.
    repo_a.commit_file("ddb/a.md", "from a", "add a").unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours has no diverging commit, so the merge fast-forwards (no merge commit).
    repo_b.fetch("origin", "master").unwrap();
    let result = repo_b.merge_remote("origin", "master").unwrap();
    assert!(
        matches!(result, MergeResult::FastForward(_)),
        "expected a fast-forward merge, got {result:?}"
    );

    // The next ordinary write carries an HLC in the wall-clock band.
    repo_b
        .commit_file("ddb/c.md", "from b", "next write")
        .unwrap();
    let next = repo_b.head_commit().unwrap();
    let hlc = crate::hlc::extract_hlc(next.message().unwrap())
        .expect("ordinary write after fast-forward must carry an HLC trailer");
    assert!(
        hlc.wall_ms > 1_600_000_000_000 && hlc.wall_ms < 10_000_000_000_000,
        "with an ordinary (non-skewed) peer the trailer must be in the wall-clock band, not a far-future constant: {}",
        hlc.wall_ms
    );
}

/// Ordinary-peer mirror of
/// `resolved_merge_commit_carries_hlc_trailer_and_absorbs_theirs`: with a NON-skewed
/// peer, the conflict-resolution merge commit's HLC trailer must land in the wall-clock
/// band, NOT a far-future constant.
#[test]
fn resolved_merge_ordinary_peer_stamps_wall_clock_band() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    // Peer (theirs, repo_a) is NOT seeded: it commits at its natural wall clock.
    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    // Ours conflicts on the SAME file with a normal clock.
    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(
        merge_commit.id().to_string(),
        merge_oid.0,
        "HEAD must be the returned resolution merge commit"
    );
    let hlc = crate::hlc::extract_hlc(merge_commit.message().unwrap())
        .expect("resolution merge commit must carry an HLC trailer");
    assert!(
        hlc.wall_ms > 1_600_000_000_000 && hlc.wall_ms < 10_000_000_000_000,
        "with an ordinary (non-skewed) peer the trailer must be in the wall-clock band, not a far-future constant: {}",
        hlc.wall_ms
    );
}

/// `commit_merge` folds a CRDT collision loser into the SAME single merge
/// commit as the winner — closing the crash window where a stranded loser
/// would live only in a second, separate commit. Fails if the loser's content
/// is dropped, landed at the wrong path/id, or landed via a SECOND commit
/// chained after the winner's (the "direct parent" check below rejects that
/// two-commit shape: a chained loser-commit would make the final HEAD's
/// parent the intermediate winner-only commit, not `head_before`).
#[test]
fn commit_merge_folds_single_loser_into_same_commit_as_winner() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let loser_content = "---\nid: 20260301120000\ntitle: Loser Note\n---\nLoser body.\n";
    let losing_blob_oid = repo_b
        .repo
        .blob(loser_content.as_bytes())
        .unwrap()
        .to_string();
    let loser = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid,
        theirs_won: true,
    };

    let expected_id =
        crate::id_minting::derive_content_id(&loser.old_id, &loser.losing_blob_oid, |_| false);
    let expected_path =
        super::doogat_path(&expected_id.0, loser.type_name.as_deref(), loser.folder);
    let expected_content =
        crate::parser::rewrite_id_field(&loser.content, &expected_id.0).unwrap();

    let head_before = repo_b.head_oid().unwrap();

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD: the returned commit must be a \
         direct child of ours' pre-merge HEAD, not a commit chained after an \
         intermediate winner-only commit"
    );

    assert_eq!(repo_b.read_file("ddb/note.md").unwrap(), "resolved");
    assert_eq!(repo_b.read_file(&expected_path).unwrap(), expected_content);
}

/// Two losers passed to the SAME `commit_merge` call must both land in that
/// one commit, at two distinct paths — neither dropped, overwritten by the
/// other, nor split across separate commits. Fails if either loser's content
/// is missing, if the two losers collide onto the same path, or if the
/// resulting HEAD is not a direct child of ours' pre-merge HEAD.
#[test]
fn commit_merge_folds_two_losers_into_distinct_paths_in_same_commit() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let loser1_content = "---\nid: 20260301120000\ntitle: Loser One\n---\nLoser one body.\n";
    let loser1_blob_oid = repo_b
        .repo
        .blob(loser1_content.as_bytes())
        .unwrap()
        .to_string();
    let loser1 = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser1_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid: loser1_blob_oid,
        theirs_won: true,
    };

    let loser2_content = "---\nid: 20260301130000\ntitle: Loser Two\n---\nLoser two body.\n";
    let loser2_blob_oid = repo_b
        .repo
        .blob(loser2_content.as_bytes())
        .unwrap()
        .to_string();
    let loser2 = crate::types::CollisionLoser {
        old_id: "20260301130000".to_string(),
        old_path: "ddb/20260301130000.md".to_string(),
        content: loser2_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid: loser2_blob_oid,
        theirs_won: true,
    };

    let expected_id1 =
        crate::id_minting::derive_content_id(&loser1.old_id, &loser1.losing_blob_oid, |_| false);
    let expected_path1 =
        super::doogat_path(&expected_id1.0, loser1.type_name.as_deref(), loser1.folder);
    let expected_content1 =
        crate::parser::rewrite_id_field(&loser1.content, &expected_id1.0).unwrap();

    let expected_id2 =
        crate::id_minting::derive_content_id(&loser2.old_id, &loser2.losing_blob_oid, |_| false);
    let expected_path2 =
        super::doogat_path(&expected_id2.0, loser2.type_name.as_deref(), loser2.folder);
    let expected_content2 =
        crate::parser::rewrite_id_field(&loser2.content, &expected_id2.0).unwrap();

    assert_ne!(
        expected_path1, expected_path2,
        "test setup invalid: the two losers must not naturally collide"
    );

    let head_before = repo_b.head_oid().unwrap();

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser1, loser2],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD for a multi-loser call"
    );

    assert_eq!(repo_b.read_file("ddb/note.md").unwrap(), "resolved");
    assert_eq!(
        repo_b.read_file(&expected_path1).unwrap(),
        expected_content1
    );
    assert_eq!(
        repo_b.read_file(&expected_path2).unwrap(),
        expected_content2
    );
}

/// A merge commit is atomic by construction: if ANY loser in a multi-loser batch
/// cannot be rewritten (here, the second loser's content has no frontmatter block
/// at all, so `rewrite_id_field` has no id field to rewrite), `commit_merge` must
/// commit NOTHING — not HEAD, not the winner, not the first (otherwise-valid)
/// loser. Landing the winner while silently dropping a loser is the half-resolved
/// data-loss shape this PRD removed; the correct behavior is to abort the whole
/// batch. Fails if the abort still lets the winner or the first loser land, or if
/// HEAD moves despite the error.
#[test]
fn commit_merge_commits_nothing_when_one_loser_in_the_batch_cannot_be_rewritten() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    // First loser: well-formed, would land successfully on its own.
    let loser1_content = "---\nid: 20260301120000\ntitle: Loser One\n---\nLoser one body.\n";
    let loser1_blob_oid = repo_b
        .repo
        .blob(loser1_content.as_bytes())
        .unwrap()
        .to_string();
    let loser1 = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser1_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid: loser1_blob_oid,
        theirs_won: true,
    };
    let expected_id1 =
        crate::id_minting::derive_content_id(&loser1.old_id, &loser1.losing_blob_oid, |_| false);
    let expected_path1 =
        super::doogat_path(&expected_id1.0, loser1.type_name.as_deref(), loser1.folder);

    // Second loser: no frontmatter block at all, so there is no id field for
    // `rewrite_id_field` to rewrite. Confirmed to fail below before relying on
    // it to trigger the abort.
    let loser2_content = "Just a plain body with no frontmatter block at all.\n";
    assert!(
        crate::parser::rewrite_id_field(loser2_content, "20260301999999").is_err(),
        "test setup invalid: rewrite_id_field must fail on frontmatter-less content"
    );
    let loser2_blob_oid = repo_b
        .repo
        .blob(loser2_content.as_bytes())
        .unwrap()
        .to_string();
    let loser2 = crate::types::CollisionLoser {
        old_id: "20260301130000".to_string(),
        old_path: "ddb/20260301130000.md".to_string(),
        content: loser2_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid: loser2_blob_oid,
        theirs_won: true,
    };

    let head_before = repo_b.head_oid().unwrap();

    let result = repo_b.commit_merge(
        &[("ddb/note.md", "resolved")],
        &[],
        &[loser1, loser2],
        "merge origin/master",
        &theirs_oid,
    );

    assert!(
        result.is_err(),
        "commit_merge must fail atomically when one loser in the batch cannot be rewritten"
    );

    let head_after = repo_b.head_oid().unwrap();
    assert_eq!(
        head_after.0, head_before.0,
        "HEAD must not move when the batch commit aborts"
    );

    assert_eq!(
        repo_b.read_file("ddb/note.md").unwrap(),
        "version B",
        "the winner's resolved content must not land when the batch aborts"
    );
    assert!(
        repo_b.read_file(&expected_path1).is_err(),
        "the first loser must not land either — the commit is atomic"
    );
}

/// Widens the loser-id occupancy check beyond the loser's own type/folder: an id
/// already used by a file under a DIFFERENT type folder must still block the
/// naive derivation and force an advance. Fails if the loser lands at the
/// naturally-derived id (whether at the occupied path or at its own-type path
/// sharing the same id), or if the occupying file's content is disturbed.
#[test]
fn commit_merge_advances_loser_past_id_occupied_in_different_type_folder() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    let loser_content = "---\nid: 20260301120000\ntitle: Loser Meeting\n---\nLoser body.\n";
    let losing_blob_oid = repo_b
        .repo
        .blob(loser_content.as_bytes())
        .unwrap()
        .to_string();
    let loser = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/meeting/20260301120000.md".to_string(),
        content: loser_content.to_string(),
        folder: true,
        type_name: Some("meeting".to_string()),
        losing_blob_oid,
        theirs_won: true,
    };

    let natural_id =
        crate::id_minting::derive_content_id(&loser.old_id, &loser.losing_blob_oid, |_| false);
    let natural_path_own_type =
        super::doogat_path(&natural_id.0, loser.type_name.as_deref(), loser.folder);

    // Occupy the natural id under a type folder that is NOT the loser's own type.
    let occupying_path = super::doogat_path(&natural_id.0, Some("project"), true);
    assert_ne!(
        occupying_path, natural_path_own_type,
        "test setup invalid: the occupying path must be under a different type \
         folder than the loser's own"
    );
    let occupying_content = format!(
        "---\nid: {}\ntitle: Occupant\n---\nOccupant body.\n",
        natural_id.0
    );
    repo_b
        .commit_file(
            &occupying_path,
            &occupying_content,
            "add occupying project doogat",
        )
        .unwrap();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let expected_advanced_id = crate::id_minting::derive_content_id(
        &loser.old_id,
        &loser.losing_blob_oid,
        |candidate| candidate == natural_id.0.as_str(),
    );
    assert_ne!(
        expected_advanced_id.0, natural_id.0,
        "test setup invalid: the naturally-derived id must really be the one \
         occupied under the foreign type folder, otherwise this degrades into a \
         no-collision test"
    );
    let expected_advanced_path = super::doogat_path(
        &expected_advanced_id.0,
        loser.type_name.as_deref(),
        loser.folder,
    );
    let expected_advanced_content =
        crate::parser::rewrite_id_field(&loser.content, &expected_advanced_id.0).unwrap();

    let head_before = repo_b.head_oid().unwrap();

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD: the returned commit must be a \
         direct child of ours' pre-merge HEAD, not a commit chained after an \
         intermediate winner-only commit"
    );

    assert_eq!(
        repo_b.read_file(&occupying_path).unwrap(),
        occupying_content,
        "the file occupying the naturally-derived id under a different type \
         folder must survive the merge unchanged"
    );
    assert!(
        repo_b.read_file(&natural_path_own_type).is_err(),
        "the loser must not be written at the naturally-derived id under its own \
         type folder either -- the id is taken repo-wide, not just at the exact \
         occupied path"
    );
    assert_eq!(
        repo_b.read_file(&expected_advanced_path).unwrap(),
        expected_advanced_content,
        "the loser must land at the advanced id with its frontmatter id rewritten"
    );
}

/// Widens the loser-id occupancy check to `ddb/_typedef/`, a shape `doogat_path`
/// cannot even produce (no `_typedef` arm -- it's written literally). An id
/// already used by a typedef file must still block the naive derivation and
/// force an advance. Fails if the loser lands at the naturally-derived id, or if
/// the typedef file's content is disturbed.
#[test]
fn commit_merge_advances_loser_past_id_occupied_in_typedef() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    let loser_content = "---\nid: 20260301120000\ntitle: Loser Note\n---\nLoser body.\n";
    let losing_blob_oid = repo_b
        .repo
        .blob(loser_content.as_bytes())
        .unwrap()
        .to_string();
    let loser = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid,
        theirs_won: true,
    };

    let natural_id =
        crate::id_minting::derive_content_id(&loser.old_id, &loser.losing_blob_oid, |_| false);
    let natural_path = super::doogat_path(&natural_id.0, loser.type_name.as_deref(), loser.folder);

    // Occupy the natural id under ddb/_typedef/ -- a shape `doogat_path` cannot
    // even produce; it's written literally.
    let occupying_path = format!("ddb/_typedef/{}.md", natural_id.0);
    assert_ne!(
        occupying_path, natural_path,
        "test setup invalid: the occupying typedef path must differ from the \
         loser's own natural path"
    );
    let occupying_content = format!(
        "---\nid: {}\ntitle: Occupant Typedef\n---\nOccupant body.\n",
        natural_id.0
    );
    repo_b
        .commit_file(&occupying_path, &occupying_content, "add occupying typedef")
        .unwrap();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let expected_advanced_id = crate::id_minting::derive_content_id(
        &loser.old_id,
        &loser.losing_blob_oid,
        |candidate| candidate == natural_id.0.as_str(),
    );
    assert_ne!(
        expected_advanced_id.0, natural_id.0,
        "test setup invalid: the naturally-derived id must really be the one \
         occupied under ddb/_typedef/, otherwise this degrades into a \
         no-collision test"
    );
    let expected_advanced_path = super::doogat_path(
        &expected_advanced_id.0,
        loser.type_name.as_deref(),
        loser.folder,
    );
    let expected_advanced_content =
        crate::parser::rewrite_id_field(&loser.content, &expected_advanced_id.0).unwrap();

    let head_before = repo_b.head_oid().unwrap();

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD: the returned commit must be a \
         direct child of ours' pre-merge HEAD, not a commit chained after an \
         intermediate winner-only commit"
    );

    assert_eq!(
        repo_b.read_file(&occupying_path).unwrap(),
        occupying_content,
        "the typedef file occupying the naturally-derived id must survive the \
         merge unchanged"
    );
    assert!(
        repo_b.read_file(&natural_path).is_err(),
        "the loser must not be written at the id already occupied under \
         ddb/_typedef/"
    );
    assert_eq!(
        repo_b.read_file(&expected_advanced_path).unwrap(),
        expected_advanced_content,
        "the loser must land at the advanced id with its frontmatter id rewritten"
    );
}

/// Two losers in the SAME `commit_merge` call whose naturally-derived ids
/// collide with EACH OTHER (nothing yet in the repo is occupied) must still
/// land at two distinct paths: the second loser has to see the first loser's
/// just-assigned id as taken. Fails if the in-batch collision tracking
/// regresses while widening the repo-wide occupancy check -- e.g. if either
/// loser's content is dropped or both land at the same path.
#[test]
fn commit_merge_advances_second_loser_past_first_losers_in_batch_assigned_id() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    // Both losers share the same old_id and the same content, so they derive
    // the SAME natural id -- an in-batch collision with nothing yet in the repo.
    let shared_old_id = "20260301120000".to_string();
    let shared_content = "---\nid: 20260301120000\ntitle: Shared Loser\n---\nShared body.\n";
    let shared_blob_oid = repo_b
        .repo
        .blob(shared_content.as_bytes())
        .unwrap()
        .to_string();

    let loser1 = crate::types::CollisionLoser {
        old_id: shared_old_id.clone(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: shared_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid: shared_blob_oid.clone(),
        theirs_won: true,
    };
    let loser2 = loser1.clone();

    let natural_id =
        crate::id_minting::derive_content_id(&shared_old_id, &shared_blob_oid, |_| false);
    let expected_id2 =
        crate::id_minting::derive_content_id(&shared_old_id, &shared_blob_oid, |candidate| {
            candidate == natural_id.0.as_str()
        });
    assert_ne!(
        expected_id2.0, natural_id.0,
        "test setup invalid: the two losers must really share a natural id, \
         otherwise this degrades into a no-collision test"
    );

    let expected_path1 =
        super::doogat_path(&natural_id.0, loser1.type_name.as_deref(), loser1.folder);
    let expected_content1 =
        crate::parser::rewrite_id_field(&loser1.content, &natural_id.0).unwrap();

    let expected_path2 =
        super::doogat_path(&expected_id2.0, loser2.type_name.as_deref(), loser2.folder);
    let expected_content2 =
        crate::parser::rewrite_id_field(&loser2.content, &expected_id2.0).unwrap();

    assert_ne!(
        expected_path1, expected_path2,
        "test setup invalid: the second loser's advanced path must differ from \
         the first loser's path"
    );

    let head_before = repo_b.head_oid().unwrap();

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser1, loser2],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD for a multi-loser call"
    );

    assert_eq!(repo_b.read_file(&expected_path1).unwrap(), expected_content1);
    assert_eq!(repo_b.read_file(&expected_path2).unwrap(), expected_content2);
}

/// Folds an identical single flat/untyped loser into an identical (freshly
/// constructed) add-add conflict on its own `setup_two_repos` node, returning
/// where the loser landed and what it holds there.
fn fold_flat_loser_and_read_result() -> (String, String) {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let loser_content = "---\nid: 20260301120000\ntitle: Loser Note\n---\nLoser body.\n";
    let losing_blob_oid = repo_b
        .repo
        .blob(loser_content.as_bytes())
        .unwrap()
        .to_string();
    let expected_id =
        crate::id_minting::derive_content_id("20260301120000", &losing_blob_oid, |_| false);
    let expected_path = super::doogat_path(&expected_id.0, None, false);

    let loser = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid,
        theirs_won: true,
    };

    let head_before = repo_b.head_oid().unwrap();

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser],
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD, not one chained after an \
         intermediate winner-only commit"
    );

    let content = repo_b.read_file(&expected_path).unwrap();
    (expected_path, content)
}

/// Two independent nodes folding the identical loser against an identically-
/// constructed merge tree must derive the identical id -- content-addressed,
/// not coordinated. Fails if the assigned id or path depends on anything
/// node-local instead of purely on the loser's content and what it's folded
/// against.
#[test]
fn commit_merge_derives_identical_id_for_identical_loser_on_independent_nodes() {
    let (path_1, content_1) = fold_flat_loser_and_read_result();
    let (path_2, content_2) = fold_flat_loser_and_read_result();

    assert_eq!(
        path_1, path_2,
        "two independent nodes folding the identical loser against an \
         identically-constructed merge tree must derive the identical id"
    );
    assert_eq!(content_1, content_2);
}

/// The collision link-rewrite scan must leave a non-UTF-8 `ddb/**.md` blob
/// byte-for-byte untouched even when it mentions the loser's old id, while
/// still rewriting a sibling VALID-UTF-8 linker to the loser's new id and
/// still landing the whole thing as one successful merge commit. Fails if
/// the non-UTF-8 blob's bytes differ at all from what was committed (the old
/// lossy-decode-then-recommit path replaces invalid bytes with U+FFFD), if
/// the merge fails outright because of the undecodable blob, or if the
/// valid-UTF-8 linker stops getting rewritten (which would mean the scan was
/// gutted to "skip everything" rather than fixed to "skip only what it can't
/// decode").
#[test]
fn commit_merge_leaves_non_utf8_linker_untouched_but_still_rewrites_valid_linker() {
    let (_dir_a, repo_a, dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();

    let loser_content = "---\nid: 20260301120000\ntitle: Loser Note\n---\nLoser body.\n";
    let losing_blob_oid = repo_b
        .repo
        .blob(loser_content.as_bytes())
        .unwrap()
        .to_string();
    let loser = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid,
        theirs_won: true,
    };

    let expected_id =
        crate::id_minting::derive_content_id(&loser.old_id, &loser.losing_blob_oid, |_| false);
    let expected_path =
        super::doogat_path(&expected_id.0, loser.type_name.as_deref(), loser.folder);
    let expected_content =
        crate::parser::rewrite_id_field(&loser.content, &expected_id.0).unwrap();

    // Valid-UTF-8 linker, committed only on repo_b (the losing side, since
    // `theirs_won: true` means repo_a is the winner). It mentions the
    // loser's old id and must still be rewritten to the loser's new id.
    let valid_linker_content = format!(
        "---\nid: 20260301999999\ntitle: Valid Linker\n---\nsee [[{}]] again\n",
        loser.old_id
    );
    repo_b
        .commit_file("ddb/linker.md", &valid_linker_content, "add valid linker")
        .unwrap();

    // Non-UTF-8 linker, committed only on repo_b. `commit_file` takes `&str`
    // and cannot hold invalid UTF-8, so this one goes through git2 directly.
    let mut invalid_bytes =
        format!("---\nid: 20991231235959\ntitle: Bad Linker\n---\nsee [[{}]] ", loser.old_id)
            .into_bytes();
    invalid_bytes.push(0xFF); // lone continuation byte: invalid at any position
    invalid_bytes.extend_from_slice(b" and more\n");

    assert!(
        std::str::from_utf8(&invalid_bytes).is_err(),
        "fixture guard: the non-UTF-8 fixture must actually be invalid UTF-8, \
         otherwise this test degrades into a plain-ASCII test"
    );
    assert!(
        invalid_bytes
            .windows(loser.old_id.len())
            .any(|w| w == loser.old_id.as_bytes()),
        "fixture guard: the non-UTF-8 fixture must contain the loser's old id \
         as bytes, otherwise the scan never has a reason to look at this file"
    );

    let abs = dir_b.path().join("ddb/non_utf8_linker.md");
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, &invalid_bytes).unwrap();

    let mut index = repo_b.repo.index().unwrap();
    index
        .add_path(std::path::Path::new("ddb/non_utf8_linker.md"))
        .unwrap();
    index.write().unwrap();
    let commit_tree = repo_b.repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("ddb", "ddb@localhost").unwrap();
    let parent = repo_b.repo.head().unwrap().peel_to_commit().unwrap();
    repo_b
        .repo
        .commit(
            Some("HEAD"),
            &sig,
            &sig,
            "add non-utf8 linker",
            &commit_tree,
            &[&parent],
        )
        .unwrap();

    repo_b.fetch("origin", "master").unwrap();
    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(
                conflicts.len(),
                1,
                "only ddb/note.md should conflict; the linker files are clean \
                 additions on repo_b's side"
            );
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let head_before = repo_b.head_oid().unwrap();

    let merge_oid = repo_b
        .commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            std::slice::from_ref(&loser),
            "merge origin/master",
            &theirs_oid,
        )
        .unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD: the merge must still \
         succeed as a single commit even with an undecodable blob in the tree"
    );

    // Winner and reassigned loser both land as they normally would.
    assert_eq!(repo_b.read_file("ddb/note.md").unwrap(), "resolved");
    assert_eq!(repo_b.read_file(&expected_path).unwrap(), expected_content);

    // The non-UTF-8 blob must be byte-for-byte untouched: not decoded, not
    // partially rewritten, not re-encoded.
    let merged_tree = repo_b.head_commit().unwrap().tree().unwrap();
    let entry = merged_tree
        .get_path(std::path::Path::new("ddb/non_utf8_linker.md"))
        .unwrap();
    let blob = repo_b.repo.find_blob(entry.id()).unwrap();
    assert_eq!(
        blob.content(),
        invalid_bytes.as_slice(),
        "non-UTF-8 blob must survive the collision link-rewrite scan byte-for-byte"
    );

    // The valid-UTF-8 sibling linker must still be rewritten -- proves the
    // scan wasn't just gutted to skip everything.
    let rewritten = repo_b.read_file("ddb/linker.md").unwrap();
    assert!(
        !rewritten.contains(&loser.old_id),
        "valid-UTF-8 linker still references the loser's old id after merge: {rewritten:?}"
    );
    assert!(
        rewritten.contains(&expected_id.0),
        "valid-UTF-8 linker was not rewritten to the loser's new id: {rewritten:?}"
    );
}

/// Captures `tracing` output emitted on the current thread. `commit_merge`
/// runs synchronously, so installing this as the thread-local default
/// subscriber for the duration of the call (via `tracing::subscriber::with_default`)
/// is enough to observe its logging without leaking into or being clobbered
/// by other tests running in parallel.
#[derive(Clone, Default)]
struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A collision loser reassigned by `commit_merge` must leave a WARN-level
/// audit trail carrying its old id, its new (derived) id, its old path, and
/// its new path -- the only durable record of an identity change that would
/// otherwise be discoverable only by diffing trees after the fact. Fails if
/// no event is captured at all (nothing logged, or logged below WARN and so
/// dropped by the WARN-level subscriber filter installed below), or if the
/// captured text is missing any of the four values.
#[test]
fn commit_merge_logs_reassignment_warn_with_old_and_new_identity() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let loser_content = "---\nid: 20260301120000\ntitle: Loser Note\n---\nLoser body.\n";
    let losing_blob_oid = repo_b
        .repo
        .blob(loser_content.as_bytes())
        .unwrap()
        .to_string();
    let loser = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid,
        theirs_won: true,
    };

    let expected_id =
        crate::id_minting::derive_content_id(&loser.old_id, &loser.losing_blob_oid, |_| false);
    let expected_path =
        super::doogat_path(&expected_id.0, loser.type_name.as_deref(), loser.folder);
    assert_ne!(
        expected_id.0, loser.old_id,
        "test setup invalid: the loser's derived id must differ from its old id, \
         otherwise this test cannot tell a reassignment record from a no-op"
    );

    let old_id = loser.old_id.clone();
    let old_path = loser.old_path.clone();

    let head_before = repo_b.head_oid().unwrap();

    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let merge_oid = tracing::subscriber::with_default(subscriber, || {
        repo_b.commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser],
            "merge origin/master",
            &theirs_oid,
        )
    })
    .unwrap();
    let captured = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD: restoring the reassignment log \
         must not come at the cost of the merge's single-commit atomicity"
    );

    assert!(
        !captured.trim().is_empty(),
        "expected a WARN-level reassignment event to be captured under a WARN-max-level \
         subscriber; got no output at all, which means either nothing was logged or it \
         was logged below WARN and filtered out"
    );
    assert!(
        captured.contains(&old_id),
        "reassignment log is missing the loser's OLD id: {captured:?}"
    );
    assert!(
        captured.contains(&expected_id.0),
        "reassignment log is missing the loser's NEW (derived) id: {captured:?}"
    );
    assert!(
        captured.contains(&old_path),
        "reassignment log is missing the loser's OLD path: {captured:?}"
    );
    assert!(
        captured.contains(&expected_path),
        "reassignment log is missing the loser's NEW path: {captured:?}"
    );
}

/// Two collision losers folded into the SAME `commit_merge` call must each
/// leave their OWN WARN-level reassignment record: both old/new id pairs and
/// both old/new path pairs must appear in the captured output. Fails an
/// implementation that logs only the first loser (or only the last), which
/// would silently drop half the audit trail for a multi-loser merge.
#[test]
fn commit_merge_logs_distinct_reassignment_warn_per_loser() {
    let (_dir_a, repo_a, _dir_b, repo_b, _bare) = setup_two_repos();

    repo_a
        .commit_file("ddb/note.md", "version A", "A edits")
        .unwrap();
    repo_a.push("origin", "master").unwrap();

    repo_b
        .commit_file("ddb/note.md", "version B", "B edits")
        .unwrap();
    repo_b.fetch("origin", "master").unwrap();

    let result = repo_b.merge_remote("origin", "master").unwrap();
    let theirs_oid = match result {
        MergeResult::Conflicts(conflicts, theirs_oid) => {
            assert_eq!(conflicts.len(), 1);
            theirs_oid
        }
        other => panic!("expected Conflicts, got {other:?}"),
    };

    let loser1_content = "---\nid: 20260301120000\ntitle: Loser One\n---\nLoser one body.\n";
    let loser1_blob_oid = repo_b
        .repo
        .blob(loser1_content.as_bytes())
        .unwrap()
        .to_string();
    let loser1 = crate::types::CollisionLoser {
        old_id: "20260301120000".to_string(),
        old_path: "ddb/20260301120000.md".to_string(),
        content: loser1_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid: loser1_blob_oid,
        theirs_won: true,
    };

    let loser2_content = "---\nid: 20260301130000\ntitle: Loser Two\n---\nLoser two body.\n";
    let loser2_blob_oid = repo_b
        .repo
        .blob(loser2_content.as_bytes())
        .unwrap()
        .to_string();
    let loser2 = crate::types::CollisionLoser {
        old_id: "20260301130000".to_string(),
        old_path: "ddb/20260301130000.md".to_string(),
        content: loser2_content.to_string(),
        folder: false,
        type_name: None,
        losing_blob_oid: loser2_blob_oid,
        theirs_won: true,
    };

    let expected_id1 =
        crate::id_minting::derive_content_id(&loser1.old_id, &loser1.losing_blob_oid, |_| false);
    let expected_path1 =
        super::doogat_path(&expected_id1.0, loser1.type_name.as_deref(), loser1.folder);
    let expected_id2 =
        crate::id_minting::derive_content_id(&loser2.old_id, &loser2.losing_blob_oid, |_| false);
    let expected_path2 =
        super::doogat_path(&expected_id2.0, loser2.type_name.as_deref(), loser2.folder);

    assert_ne!(
        expected_id1.0, loser1.old_id,
        "test setup invalid: loser1's derived id must differ from its old id"
    );
    assert_ne!(
        expected_id2.0, loser2.old_id,
        "test setup invalid: loser2's derived id must differ from its old id"
    );
    assert_ne!(
        expected_id1.0, expected_id2.0,
        "test setup invalid: the two losers must not derive the same new id"
    );

    let old_id1 = loser1.old_id.clone();
    let old_path1 = loser1.old_path.clone();
    let old_id2 = loser2.old_id.clone();
    let old_path2 = loser2.old_path.clone();

    let head_before = repo_b.head_oid().unwrap();

    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();
    let merge_oid = tracing::subscriber::with_default(subscriber, || {
        repo_b.commit_merge(
            &[("ddb/note.md", "resolved")],
            &[],
            &[loser1, loser2],
            "merge origin/master",
            &theirs_oid,
        )
    })
    .unwrap();
    let captured = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();

    let merge_commit = repo_b.head_commit().unwrap();
    assert_eq!(merge_commit.id().to_string(), merge_oid.0);
    assert!(
        merge_commit
            .parents()
            .any(|p| p.id().to_string() == head_before.0),
        "expected exactly one new commit on HEAD for a multi-loser call: restoring \
         the reassignment log must not come at the cost of the merge's single-commit \
         atomicity"
    );

    assert!(
        captured.contains(&old_id1) && captured.contains(&expected_id1.0),
        "reassignment log is missing loser1's old id or its new (derived) id: {captured:?}"
    );
    assert!(
        captured.contains(&old_path1) && captured.contains(&expected_path1),
        "reassignment log is missing loser1's old path or its new path: {captured:?}"
    );
    assert!(
        captured.contains(&old_id2) && captured.contains(&expected_id2.0),
        "reassignment log is missing loser2's old id or its new (derived) id: {captured:?}"
    );
    assert!(
        captured.contains(&old_path2) && captured.contains(&expected_path2),
        "reassignment log is missing loser2's old path or its new path: {captured:?}"
    );
}
