use super::*;
use crate::git_ops::GitRepo;
use crate::traits::GitHistory;

fn temp_repo() -> (::tempfile::TempDir, GitRepo) {
    let dir = ::tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.repo
        .config()
        .unwrap()
        .set_bool("commit.gpgsign", false)
        .unwrap();
    (dir, repo)
}

/// Clone `source` into a fresh temp dir, so both repos share every commit made
/// so far as a real merge base. Counterpart to [`temp_repo`], which makes an
/// unrelated repo instead.
fn cloned_repo(source: &::tempfile::TempDir) -> (::tempfile::TempDir, GitRepo) {
    let dir = ::tempfile::TempDir::new().unwrap();
    git2::Repository::clone(source.path().to_str().unwrap(), dir.path()).unwrap();
    let repo = GitRepo::open(dir.path()).unwrap();
    repo.repo
        .config()
        .unwrap()
        .set_bool("commit.gpgsign", false)
        .unwrap();
    (dir, repo)
}

#[test]
fn full_bundle_export_and_verify() {
    let (_dir, repo) = temp_repo();
    repo.commit_file(
        "ddb/20260301000000.md",
        "---\ntitle: test\n---\nBody",
        "add",
    )
    .unwrap();
    crate::sync_manager::register_node(&repo, "Node1").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let output = _dir.path().join("test.bundle.tar");
    let path = export_full_bundle(&repo, &mgr, &output).unwrap();
    assert!(path.exists());

    let manifest = verify_bundle(&path).unwrap();
    assert_eq!(manifest.target_node, "*");
    assert_eq!(manifest.format_version, 1);
}

#[test]
fn checksum_verification_catches_tampering() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/20260301000000.md", "---\ntitle: test\n---\n", "add")
        .unwrap();
    crate::sync_manager::register_node(&repo, "Node1").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let output = _dir.path().join("test.bundle.tar");
    export_full_bundle(&repo, &mgr, &output).unwrap();

    // Tamper with the tar: extract, modify, repack
    let tamper_dir = _dir.path().join("tampered");
    std::fs::create_dir_all(&tamper_dir).unwrap();
    let file = std::fs::File::open(&output).unwrap();
    let mut archive = tar::Archive::new(file);
    archive.unpack(&tamper_dir).unwrap();

    // Modify manifest
    let manifest_path = tamper_dir.join("manifest.toml");
    let mut content = std::fs::read_to_string(&manifest_path).unwrap();
    content.push_str("\n# tampered\n");
    std::fs::write(&manifest_path, content).unwrap();

    // Repack
    let tampered_output = _dir.path().join("tampered.bundle.tar");
    let tar_file = std::fs::File::create(&tampered_output).unwrap();
    let mut builder = tar::Builder::new(tar_file);
    for entry in std::fs::read_dir(&tamper_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if entry.file_type().unwrap().is_dir() {
            builder
                .append_dir_all(name.to_string_lossy().as_ref(), entry.path())
                .unwrap();
        } else {
            builder
                .append_path_with_name(entry.path(), name.to_string_lossy().as_ref())
                .unwrap();
        }
    }
    builder.finish().unwrap();

    let result = verify_bundle(&tampered_output);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("checksum mismatch"));
}

#[test]
fn full_bundle_import_on_new_repo() {
    // Node 1: create content and export
    let (_dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\ntitle: test\n---\nBody",
            "add",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();

    let bundle_path = _dir1.path().join("full.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

    // Node 2: import
    let (_dir2, repo2) = temp_repo();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let db_path = _dir2.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index2 = crate::indexer::Index::open(&db_path).unwrap();

    let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path).unwrap();
    assert_eq!(report.direction, "bundle-import");

    // Verify content was imported
    let content = repo2.read_file("ddb/20260301000000.md").unwrap();
    assert!(content.contains("title: test"));
}

#[test]
fn conflicting_full_bundle_import_resolves_with_real_merge_commit() {
    // Node 1: create the shared ancestor commit.
    let (dir1, repo1) = temp_repo();
    let path = "ddb/20260301000000.md";
    repo1
        .commit_file(
            path,
            "---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n",
            "add ancestor",
        )
        .unwrap();

    // Node 2: clone Node 1's repo at this point so both nodes share the
    // ancestor commit as a real merge base.
    let (dir2, repo2) = cloned_repo(&dir1);

    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();

    // Both nodes edit the SAME line of the SAME doogat differently,
    // diverging from the shared ancestor commit above -- a real conflict.
    repo1
        .commit_file(
            path,
            "---\nid: 20260301000000\ntitle: Ancestor\n---\nNode1 body\n",
            "Node1 edits",
        )
        .unwrap();
    repo2
        .commit_file(
            path,
            "---\nid: 20260301000000\ntitle: Ancestor\n---\nNode2 body\n",
            "Node2 edits",
        )
        .unwrap();

    // Node1 exports a full bundle; Node2 imports it.
    let bundle_path = dir1.path().join("conflict.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

    let db2 = dir2.path().join(".ddb/index.db");
    std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
    let index2 = crate::indexer::Index::open(&db2).unwrap();

    // A ref outside this payload's namespace distinguishes namespaced cleanup
    // from a sweep of the whole `refs/remotes/bundle/` prefix.
    let sibling = "refs/remotes/bundle/other/master".to_string();
    let sibling_oid = repo2.repo.head().unwrap().target().unwrap();
    repo2
        .repo
        .reference(&sibling, sibling_oid, false, "sibling")
        .unwrap();

    let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path)
        .expect("a resolvable conflict must not fail the import");

    assert!(
        report.conflicts_resolved > 0,
        "a real conflict must be counted as resolved, not silently dropped (got {})",
        report.conflicts_resolved
    );

    let head = repo2.head_oid().unwrap().0;
    assert_eq!(
        repo2.commit_parent_count(&head).unwrap(),
        2,
        "a resolved conflict must land in a real 2-parent merge commit"
    );

    assert_eq!(
        bundle_refs(dir2.path()),
        vec![(sibling, sibling_oid)],
        "the bundle ref must be deleted after a successful import, and only its own"
    );

    let content = repo2.read_file(path).unwrap();
    assert!(
        content.contains("id: 20260301000000"),
        "the resolved doogat must still be readable at HEAD, got: {content}"
    );
}

/// Losing-side content whose frontmatter block holds invalid YAML: it HAS a
/// block (so it is not folded verbatim), but `rewrite_id_field` cannot parse
/// it, so `commit_merge` must abort the whole merge commit.
const UNREWRITABLE_LOSER: &str = "---\nid: [unclosed\ntitle: Bad\n---\nLoser body.\n";

/// The winning side of the add/add collision `import_with_collision` builds.
const COLLISION_WINNER: &str = "---\nid: 20260302000000\ntitle: Winner\n---\nWinner body\n";

/// Drive a bundle import whose merge FAILS: node 2 imports a bundle holding a
/// doogat that collides on id with a local one, whose losing side has
/// YAML-invalid frontmatter, so the collision loser cannot be rewritten. By
/// design this leaves `refs/remotes/bundle/<payload-id>/master` in place for
/// a retry. Returns node 1's and node 2's temp dirs; node 2's repo is
/// reopened by the caller so no borrow of it escapes this helper.
fn import_with_unresolvable_collision(
) -> (::tempfile::TempDir, ::tempfile::TempDir, Result<SyncReport>) {
    assert!(
        crate::parser::rewrite_id_field(UNREWRITABLE_LOSER, "20260302999999").is_err(),
        "test setup invalid: rewrite_id_field must fail on YAML-invalid frontmatter"
    );
    import_with_collision(UNREWRITABLE_LOSER)
}

/// Drive a bundle import over a real add/add collision at
/// `ddb/20260302000000.md` in which node 1's `loser_content` loses to node
/// 2's `COLLISION_WINNER`. Returns node 1's and node 2's temp dirs and the
/// import result.
fn import_with_collision(
    loser_content: &str,
) -> (::tempfile::TempDir, ::tempfile::TempDir, Result<SyncReport>) {
    // Node 1: create the shared ancestor commit.
    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n",
            "add ancestor",
        )
        .unwrap();

    // Node 2: clone Node 1's repo at this point so both nodes share the
    // ancestor commit as a real merge base.
    let (dir2, repo2) = cloned_repo(&dir1);

    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();

    // Both nodes independently add a NEW doogat at the SAME path, absent
    // from the shared ancestor -- a real add/add collision. The losing
    // side is Node1, "theirs" from repo2's merge-remote perspective.
    let collision_path = "ddb/20260302000000.md";

    // Node1 (theirs) is seeded with a strictly LOWER HLC so it loses the
    // add/add collision to Node2 (ours) under lww_pick's tie-break rule.
    let theirs_seed = crate::hlc::Hlc {
        wall_ms: u64::MAX / 2,
        counter: 0,
        node: "theirsss".into(),
    };
    std::fs::write(dir1.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
    repo1
        .commit_file(collision_path, loser_content, "Node1 adds loser")
        .unwrap();

    let ours_seed = crate::hlc::Hlc {
        wall_ms: u64::MAX / 2 + 1_000_000,
        counter: 0,
        node: "oursssss".into(),
    };
    std::fs::write(dir2.path().join(".git/ddb-hlc"), ours_seed.to_string()).unwrap();
    repo2
        .commit_file(collision_path, COLLISION_WINNER, "Node2 adds winner")
        .unwrap();

    // Node1 exports a full bundle; Node2 imports it.
    let bundle_path = dir1.path().join("collision.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

    let db2 = dir2.path().join(".ddb/index.db");
    std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
    let index2 = crate::indexer::Index::open(&db2).unwrap();

    let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);

    (dir1, dir2, result)
}

/// Reachability: a bundle-import merge failure (add/add collision loser with
/// YAML-invalid frontmatter, so `rewrite_id_field` cannot rewrite its id)
/// must leave `refs/remotes/bundle/<payload-id>/master` in place. `import_bundle`
/// only deletes that namespace after a successful import, so bundle data must
/// stay reachable for a retry when the merge itself fails.
#[test]
fn conflicting_bundle_import_leaves_bundle_ref_reachable_on_merge_failure() {
    let (dir1, dir2, result) = import_with_unresolvable_collision();
    let kept_ref = format!(
        "refs/remotes/bundle/{}/master",
        payload_id_of(&dir1.path().join("collision.bundle.tar"))
    );
    let err = result.expect_err("an unresolvable add/add collision must fail the import");
    assert!(
        matches!(err, DoogatError::Sync(_)),
        "an unresolvable add/add collision must fail the import with a Sync error, got {err:?}"
    );
    assert!(
        err.to_string().contains("bundle merge failed"),
        "every bundle-import failure is a Sync, so only the message proves the MERGE itself \
         failed, got: {err}"
    );

    let repo2 = GitRepo::open(dir2.path()).unwrap();
    assert!(
        repo2.repo.find_reference(&kept_ref).is_ok(),
        "the bundle ref must survive a failed import so its data stays reachable"
    );
    assert!(
        repo2.repo.revparse_single(&kept_ref).is_ok(),
        "the bundle ref must still resolve to the unbundled commits after a failed import"
    );
}

/// Clean-repo-on-error: the bundle-import merge path never shells out to
/// CLI `git merge` (`merge_remote`/`merge_commits` compute the merge
/// entirely in memory), so a merge failure must never leave a MERGE_HEAD
/// file or unmerged index entries behind -- there is no `git merge
/// --abort` step because none is needed.
#[test]
fn conflicting_bundle_import_leaves_repo_clean_on_merge_failure() {
    let (_dir1, dir2, result) = import_with_unresolvable_collision();
    let err = result.expect_err("an unresolvable add/add collision must fail the import");
    assert!(
        matches!(err, DoogatError::Sync(_)),
        "an unresolvable add/add collision must fail the import with a Sync error, got {err:?}"
    );
    assert!(
        err.to_string().contains("bundle merge failed"),
        "every bundle-import failure is a Sync, so only the message proves the MERGE itself \
         failed, got: {err}"
    );

    assert!(
        !dir2.path().join(".git/MERGE_HEAD").exists(),
        "no CLI `git merge` runs on the bundle-import path, so no MERGE_HEAD file should ever exist"
    );
    // `GitRepo::index()` returns a process-cached index that the in-memory
    // merge never writes, so a fresh on-disk handle is the only way this
    // assertion can actually observe a conflicted index.
    let reopened = git2::Repository::open(dir2.path()).unwrap();
    assert!(
        !reopened.index().unwrap().has_conflicts(),
        "an in-memory merge failure must never leave unmerged entries in the repo index"
    );
}

/// A bundle import whose add/add collision loser has NO frontmatter block
/// must succeed: the loser's bytes land verbatim at a derived path and the
/// report lists exactly that one fold. Fails on the old wedge, where the
/// import erred with "no frontmatter opening ---" on every retry.
#[test]
fn frontmatterless_bundle_loser_imports_and_reports_one_verbatim_fold() {
    let collision_path = "ddb/20260302000000.md";
    let loser_content = "Just a plain body with no frontmatter block at all.\n";
    let (_dir1, dir2, result) = import_with_collision(loser_content);
    let report = result.expect("a frontmatter-less collision loser must not fail the import");
    let repo2 = GitRepo::open(dir2.path()).unwrap();

    assert_eq!(report.collisions_reassigned, 1);
    assert_eq!(
        report.collision_losers_kept_verbatim.len(),
        1,
        "{:?}",
        report.collision_losers_kept_verbatim
    );
    let kept = &report.collision_losers_kept_verbatim[0];
    assert_eq!(kept.old_id, "20260302000000");
    assert_eq!(kept.old_path, collision_path);
    assert_ne!(kept.new_path, collision_path);
    assert_eq!(repo2.read_file(collision_path).unwrap(), COLLISION_WINNER);
    assert_eq!(repo2.read_file(&kept.new_path).unwrap(), loser_content);
}

#[test]
fn delta_export_targets_node_and_uses_known_heads() {
    let (_dir, repo) = temp_repo();

    // Create initial content
    repo.commit_file(
        "ddb/20260301000000.md",
        "---\ntitle: first\n---\nBody1",
        "add first",
    )
    .unwrap();
    crate::sync_manager::register_node(&repo, "Node1").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    // Record current head as node2's sync point
    let sync_point = repo.head_oid().unwrap().to_string();

    // Register a remote node with known_heads at sync_point
    let node2_uuid = "remote-node-2";
    let node2_config = format!(
        "uuid = \"{node2_uuid}\"\nname = \"Node2\"\nknown_heads = [\"{sync_point}\"]\n\
         status = \"Active\"\n"
    );
    repo.commit_file(
        &format!(".nodes/{node2_uuid}.toml"),
        &node2_config,
        "register node2",
    )
    .unwrap();

    // Add new content after node2's sync point
    repo.commit_file(
        "ddb/20260302000000.md",
        "---\ntitle: second\n---\nBody2",
        "add second",
    )
    .unwrap();

    // Export delta bundle targeting node2
    let output = _dir.path().join("delta.bundle.tar");
    let path = export_bundle(&repo, &mgr, node2_uuid, &output).unwrap();
    assert!(path.exists());

    // Verify manifest targets the specific node (not "*" like full export)
    let manifest = verify_bundle(&path).unwrap();
    assert_eq!(manifest.target_node, node2_uuid);
    assert_eq!(manifest.format_version, 1);

    // Verify the delta bundle is smaller than a full export
    let full_output = _dir.path().join("full.bundle.tar");
    export_full_bundle(&repo, &mgr, &full_output).unwrap();
    let delta_size = std::fs::metadata(&path).unwrap().len();
    let full_size = std::fs::metadata(&full_output).unwrap().len();
    assert!(
        delta_size < full_size,
        "delta ({delta_size}B) should be smaller than full ({full_size}B)"
    );
}

#[test]
fn delta_export_fails_for_unknown_node() {
    let (_dir, repo) = temp_repo();
    repo.commit_file("ddb/20260301000000.md", "---\ntitle: test\n---\n", "add")
        .unwrap();
    crate::sync_manager::register_node(&repo, "Node1").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    let output = _dir.path().join("delta.bundle.tar");
    let result = export_bundle(&repo, &mgr, "nonexistent-uuid", &output);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("nonexistent-uuid"));
}

/// Export a full bundle whose branch set deliberately EXCLUDES `master`,
/// so a pruning fetch would have other bundle refs to prune and the import
/// has no delivered `master` to merge.
/// Returns the OID the exported `branch` names.
fn export_bundle_without_master(output: &Path, branch: &str) -> git2::Oid {
    let (_dir, repo) = temp_repo();
    repo.commit_file(
        "ddb/20260401000000.md",
        "---\nid: 20260401000000\ntitle: Sidecar\n---\nSidecar body\n",
        "add sidecar doogat",
    )
    .unwrap();
    crate::sync_manager::register_node(&repo, "Node3").unwrap();
    let mgr = SyncManager::open(&repo).unwrap();

    // Rehome the history onto a non-master branch, then drop `master`.
    let head = repo.repo.head().unwrap().peel_to_commit().unwrap();
    repo.repo.branch(branch, &head, false).unwrap();
    repo.repo.set_head(&format!("refs/heads/{branch}")).unwrap();
    let mut master = repo
        .repo
        .find_branch("master", git2::BranchType::Local)
        .unwrap();
    master.delete().unwrap();

    export_full_bundle(&repo, &mgr, output).unwrap();
    head.id()
}

/// Every `(name, oid)` under `refs/remotes/bundle/`, sorted, from [`all_refs`]
/// (a fresh handle) so no namespace spelling below that prefix is assumed.
/// Enumeration, name and target errors fail the test instead of being skipped.
fn bundle_refs(repo_dir: &Path) -> Vec<(String, git2::Oid)> {
    all_refs(repo_dir)
        .into_iter()
        .filter(|(name, _)| name.starts_with("refs/remotes/bundle/"))
        .map(|(name, oid)| {
            let oid = oid.unwrap_or_else(|| panic!("bundle ref {name} must be direct"));
            (name, oid)
        })
        .collect()
}

/// A failed import A keeps its master ref for a retry. Once A's conflict is
/// repaired locally (without importing A), a LATER masterless bundle B must
/// not pick up A's stale master and merge it: B delivered no master, so the
/// import fails as a merge failure, HEAD and data stay put, A's kept ref
/// survives exactly, and B's fetched heads stay reachable. Checked under both
/// `fetch.prune` settings.
#[test]
fn failed_bundle_then_masterless_bundle_never_merges_stale_master() {
    let collision_path = "ddb/20260302000000.md";
    for prune in [true, false] {
        let (dir1, dir2, result) = import_with_unresolvable_collision();
        assert!(
            matches!(result, Err(DoogatError::Sync(_))),
            "setup invalid: the collision import A must fail, got {result:?}"
        );
        let stale_master = git2::Repository::open(dir1.path())
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();

        let repo2 = GitRepo::open(dir2.path()).unwrap();
        repo2
            .repo
            .config()
            .unwrap()
            .set_bool("fetch.prune", prune)
            .unwrap();
        let failed_refs = bundle_refs(dir2.path());
        assert!(
            failed_refs.iter().any(|(_, oid)| *oid == stale_master),
            "setup invalid: failed import A must keep a ref to its master, got {failed_refs:?}"
        );

        // Repair the collision locally with A's exact content, so A's master
        // would now merge cleanly, without importing A.
        repo2
            .commit_file(
                collision_path,
                UNREWRITABLE_LOSER,
                "repair collision locally",
            )
            .unwrap();
        {
            let ours = repo2.repo.head().unwrap().peel_to_commit().unwrap();
            let theirs = repo2.repo.find_commit(stale_master).unwrap();
            assert!(
                !repo2
                    .repo
                    .merge_commits(&ours, &theirs, None)
                    .unwrap()
                    .has_conflicts(),
                "setup invalid: A's master must now be mergeable"
            );
        }

        let head_before = repo2.head_oid().unwrap().0;
        let collision_before = repo2.read_file(collision_path).unwrap();

        let masterless = dir2.path().join("masterless.bundle.tar");
        let b_head = export_bundle_without_master(&masterless, "sidecar");

        let mut mgr2 = SyncManager::open(&repo2).unwrap();
        let index2 = open_index(dir2.path());
        let err = import_bundle(&repo2, &mut mgr2, &index2, &masterless).expect_err(
            "a masterless bundle must fail, never merge the stale master a failed import kept",
        );
        assert!(
            matches!(err, DoogatError::Sync(_)),
            "prune={prune}: a masterless import must fail as Sync, got {err:?}"
        );
        assert!(
            err.to_string().contains("bundle merge failed:"),
            "prune={prune}: a masterless import must report a merge failure, got: {err}"
        );
        assert!(
            err.to_string().contains(&payload_id_of(&masterless))
                && err.to_string().contains("sidecar"),
            "prune={prune}: the error must name B's payload id and delivered branch, got: {err}"
        );

        let reopened = GitRepo::open(dir2.path()).unwrap();
        assert_eq!(
            reopened.head_oid().unwrap().0,
            head_before,
            "prune={prune}: a failed masterless import must not move HEAD"
        );
        assert_eq!(
            reopened.read_file(collision_path).unwrap(),
            collision_before,
            "prune={prune}: local data must be unchanged"
        );
        assert!(
            reopened.read_file("ddb/20260401000000.md").is_err(),
            "prune={prune}: B's data must not land without a delivered master"
        );

        let after = bundle_refs(dir2.path());
        for kept in &failed_refs {
            assert!(
                after.contains(kept),
                "prune={prune}: A's kept ref {kept:?} must survive exactly, got {after:?}"
            );
        }
        assert!(
            after.iter().any(|(_, oid)| *oid == b_head),
            "prune={prune}: B's fetched head must stay reachable, got {after:?}"
        );
    }
}

/// Cleanup ordering: fetched refs are deleted only after ALL post-merge work
/// succeeds. A genuine fast-forward lands, then the final index rebuild fails
/// (read-only index), so the import must error and keep its fetched ref.
/// HEAD rollback after a clean fast-forward is not promised, so not asserted.
#[test]
fn bundle_import_keeps_refs_when_post_merge_work_fails() {
    let (dir2, repo2) = temp_repo();
    repo2
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: Base\n---\nBase body\n",
            "add base",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();

    // Node 1 descends from node 2's HEAD, so importing it is a fast-forward.
    let (dir1, repo1) = cloned_repo(&dir2);
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    repo1
        .commit_file(
            "ddb/20260303000000.md",
            "---\nid: 20260303000000\ntitle: Ahead\n---\nAhead body\n",
            "add ahead",
        )
        .unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let bundle_path = dir1.path().join("ff.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();
    let bundle_head = repo1.repo.head().unwrap().target().unwrap();
    let local_head = repo2.repo.head().unwrap().target().unwrap();
    assert!(
        repo1
            .repo
            .graph_descendant_of(bundle_head, local_head)
            .unwrap(),
        "setup invalid: the bundle must fast-forward node 2"
    );

    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = open_index(dir2.path());
    index2
        .conn
        .execute_batch("PRAGMA query_only = ON;")
        .unwrap();

    let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
    assert!(
        result.is_err(),
        "a failed final index rebuild must fail the import, got {result:?}"
    );
    assert_eq!(
        GitRepo::open(dir2.path()).unwrap().head_oid().unwrap().0,
        bundle_head.to_string(),
        "setup invalid: the fast-forward must have landed before the rebuild failed"
    );

    let after = bundle_refs(dir2.path());
    assert!(
        after.iter().any(|(_, oid)| *oid == bundle_head),
        "post-merge failure must keep the fetched ref to the bundle head, got {after:?}"
    );
}

/// Reachability under `fetch.prune`: a failed import keeps
/// `refs/remotes/bundle/<payload-id>/master` so its data stays reachable for a
/// retry.
/// A LATER import of a different bundle must not silently take that
/// reachability away, even when the repo has `fetch.prune = true` (a
/// common global git setting) and the new bundle carries no `master`.
#[test]
fn kept_bundle_ref_survives_a_later_import_under_fetch_prune() {
    let (dir1, dir2, result) = import_with_unresolvable_collision();
    let kept_ref = format!(
        "refs/remotes/bundle/{}/master",
        payload_id_of(&dir1.path().join("collision.bundle.tar"))
    );
    assert!(
        matches!(result, Err(DoogatError::Sync(_))),
        "setup invalid: the collision import must fail, got {result:?}"
    );
    let repo2 = GitRepo::open(dir2.path()).unwrap();
    repo2
        .repo
        .config()
        .unwrap()
        .set_bool("fetch.prune", true)
        .unwrap();

    let kept = repo2
        .repo
        .revparse_single(&kept_ref)
        .expect("setup invalid: the failed import must have kept the bundle ref")
        .id();

    let other_bundle = dir2.path().join("other.bundle.tar");
    export_bundle_without_master(&other_bundle, "sidecar");

    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = crate::indexer::Index::open(&dir2.path().join(".ddb/index.db")).unwrap();
    // Whether this second import succeeds is not the property under test;
    // the first bundle's data staying reachable is.
    let _ = import_bundle(&repo2, &mut mgr2, &index2, &other_bundle);

    let survivor = repo2
        .repo
        .revparse_single(&kept_ref)
        .expect("a later import must not prune the bundle ref a failed import kept");
    assert_eq!(
        survivor.id(),
        kept,
        "the kept bundle ref must still name the first bundle's commit"
    );
    assert!(
        repo2.repo.find_commit(kept).is_ok(),
        "the commit the kept bundle ref names must still be present"
    );
}

/// Cleanup completeness: the import fetch maps `refs/heads/*` onto
/// `refs/remotes/bundle/<payload-id>/*`, so a multi-branch bundle creates
/// several bundle refs. A successful import must clear its whole namespace,
/// not just `master`, or stale bundle refs pile up in the repo.
#[test]
fn successful_import_deletes_every_bundle_ref_not_just_master() {
    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: test\n---\nBody",
            "add",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();

    // A second branch besides `master`, so the bundle carries two heads.
    let head1 = repo1.repo.head().unwrap().peel_to_commit().unwrap();
    repo1.repo.branch("feature", &head1, false).unwrap();

    let bundle_path = dir1.path().join("multi-branch.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

    let (dir2, repo2) = temp_repo();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let db2 = dir2.path().join(".ddb/index.db");
    std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
    let index2 = crate::indexer::Index::open(&db2).unwrap();

    let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path)
        .expect("a clean multi-branch bundle must import successfully");
    assert_eq!(report.direction, "bundle-import");
    let content = repo2.read_file("ddb/20260301000000.md").unwrap();
    assert!(
        content.contains("title: test"),
        "the imported doogat must be readable at HEAD, got: {content}"
    );

    let leftover = bundle_refs(dir2.path());
    assert!(
        leftover.is_empty(),
        "a successful import must delete every bundle ref, not just master; leftover: {leftover:?}"
    );
}

/// Two separately `init`-ed repos have UNRELATED histories, so every
/// conflicting path arrives with `ancestor: None`. A conflicting
/// NON-doogat file (`.gitignore`) must be resolved by the ordinary
/// conflict path; routing it into the doogat add/add collision resolver
/// kills the documented full-bundle bootstrap with an unrecoverable
/// frontmatter parse error.
#[test]
fn unrelated_history_import_resolves_conflicting_non_doogat_file() {
    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(".gitignore", "node_modules/\n", "Node1 gitignore")
        .unwrap();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: Node1 doc\n---\nNode1 body\n",
            "Node1 doogat",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();

    let bundle_path = dir1.path().join("unrelated-nondoogat.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

    // Node 2 is initialized on its own -- NOT cloned -- so the two
    // histories share no merge base.
    let (dir2, repo2) = temp_repo();
    repo2
        .commit_file(".gitignore", "target/\n", "Node2 gitignore")
        .unwrap();
    repo2
        .commit_file(
            "ddb/20260302000000.md",
            "---\nid: 20260302000000\ntitle: Node2 doc\n---\nNode2 body\n",
            "Node2 doogat",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let db2 = dir2.path().join(".ddb/index.db");
    std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
    let index2 = crate::indexer::Index::open(&db2).unwrap();

    let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
    if let Err(err) = &result {
        let message = err.to_string();
        assert!(
            !message.contains("could not be rewritten: parse: no frontmatter opening ---"),
            "a conflicting non-doogat file must not be routed through the doogat add/add collision resolver, got: {message}"
        );
    }
    let report =
        result.expect("an unrelated-history import with a conflicting .gitignore must succeed");
    assert_eq!(report.direction, "bundle-import");
    assert_eq!(
        report.collisions_reassigned, 0,
        "no doogat id collides here, so nothing may be reassigned"
    );

    let gitignore = repo2.read_file(".gitignore").unwrap();
    assert!(
        gitignore.contains("target/") || gitignore.contains("node_modules/"),
        "the conflicting .gitignore must keep one side's content, got: {gitignore}"
    );

    assert!(repo2
        .read_file("ddb/20260301000000.md")
        .unwrap()
        .contains("Node1 body"));
    assert!(repo2
        .read_file("ddb/20260302000000.md")
        .unwrap()
        .contains("Node2 body"));
}

/// Same unrelated-history shape, but both sides independently hold the
/// SAME doogat id. That IS a genuine add/add collision: both documents
/// must survive and the reassigned loser must get a real 14-digit
/// `YYYYMMDDHHmmss` id, never a silent duplicate under an invalid one.
#[test]
fn unrelated_history_import_reassigns_colliding_doogat_to_valid_id() {
    let path = "ddb/20260301000000.md";

    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            path,
            "---\nid: 20260301000000\ntitle: Node1\n---\nNode1 body\n",
            "Node1 adds",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();

    let bundle_path = dir1.path().join("unrelated-collision.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

    // Node 2 is initialized on its own -- NOT cloned -- so the two
    // histories share no merge base.
    let (dir2, repo2) = temp_repo();
    repo2
        .commit_file(
            path,
            "---\nid: 20260301000000\ntitle: Node2\n---\nNode2 body\n",
            "Node2 adds",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let db2 = dir2.path().join(".ddb/index.db");
    std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
    let index2 = crate::indexer::Index::open(&db2).unwrap();

    let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path)
        .expect("a same-id collision across unrelated histories must resolve, not fail");
    assert_eq!(report.direction, "bundle-import");
    assert!(
        report.collisions_reassigned > 0,
        "the duplicated doogat id must be reported as a reassignment, got {}",
        report.collisions_reassigned
    );

    let tree = repo2
        .repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let ddb_entry = tree
        .get_name("ddb")
        .expect("the ddb/ directory must exist at HEAD after an import");
    let ddb_tree = repo2.repo.find_tree(ddb_entry.id()).unwrap();
    let stems: Vec<String> = ddb_tree
        .iter()
        .filter_map(|entry| entry.name().ok().map(str::to_string))
        .filter_map(|name| name.strip_suffix(".md").map(str::to_string))
        .collect();

    assert_eq!(
        stems.len(),
        2,
        "both colliding documents must survive the reassignment, got {stems:?}"
    );
    for stem in &stems {
        assert!(
            crate::types::DoogatId::is_calendar_shaped(stem),
            "every surviving doogat must live under a valid 14-digit YYYYMMDDHHmmss id, got ddb/{stem}.md"
        );
    }

    let bodies: Vec<String> = stems
        .iter()
        .map(|stem| repo2.read_file(&format!("ddb/{stem}.md")).unwrap())
        .collect();
    assert!(
        bodies.iter().any(|body| body.contains("Node1 body")),
        "the imported side must survive, got {bodies:?}"
    );
    assert!(
        bodies.iter().any(|body| body.contains("Node2 body")),
        "the local side must survive, got {bodies:?}"
    );
    for (stem, body) in stems.iter().zip(&bodies) {
        assert!(
            body.contains(&format!("id: {stem}")),
            "a reassigned doogat's frontmatter id must match its filename, got ddb/{stem}.md: {body}"
        );
    }
}

/// A `Conflict` from the merge sequence is wrapped as `Sync` like every
/// other variant. `Conflict` is NOT a reliable "retryable" marker here: the
/// merge path raises it both for genuinely retryable failures (write-lock
/// acquire timeout, the resolve→commit window guard) and for terminal ones
/// (a collision loser whose id cannot be rewritten). Either wrapped call can
/// raise either class, and the terminal case is exactly what
/// `conflicting_bundle_import_leaves_repo_clean_on_merge_failure` and
/// `kept_bundle_ref_survives_a_later_import_under_fetch_prune` require to be
/// reported as `Sync`. Letting `Conflict` through would break the documented
/// "every bundle-import failure is a `Sync`" contract.
#[test]
fn bundle_merge_error_wraps_conflict_as_sync_because_conflict_is_not_retryable_here() {
    let message = match bundle_merge_error(DoogatError::Conflict(
        "collision loser at ddb/x.md could not be rewritten".to_string(),
    )) {
        DoogatError::Sync(msg) => msg,
        other => panic!("a merge-path Conflict must be wrapped as Sync, got {other:?}"),
    };
    assert!(
        message.starts_with("bundle merge failed: "),
        "message must start with the exact prefix, got: {message}"
    );
    assert!(
        message.contains("collision loser at ddb/x.md could not be rewritten"),
        "original error text must be preserved, got: {message}"
    );
}

#[test]
fn bundle_merge_error_wraps_every_variant_as_sync_with_prefix() {
    let cases: Vec<(DoogatError, &str)> = vec![
        (
            DoogatError::NotFound("refs/remotes/bundle/master".to_string()),
            "refs/remotes/bundle/master",
        ),
        (
            DoogatError::Git("failed to fetch objects".to_string()),
            "failed to fetch objects",
        ),
        (
            DoogatError::Validation("bad frontmatter".to_string()),
            "bad frontmatter",
        ),
    ];

    for (input, original_text) in cases {
        let message = match bundle_merge_error(input) {
            DoogatError::Sync(msg) => msg,
            other => panic!("every merge-path error must be wrapped as Sync, got {other:?}"),
        };
        assert!(
            message.starts_with("bundle merge failed: "),
            "message must start with the exact prefix, got: {message}"
        );
        assert!(
            message.contains(original_text),
            "original error text must be preserved, got: {message}"
        );
    }
}

/// Reproduces the live failure: a bundle exported from a `main`-branch
/// repo carries `refs/heads/main` but no `refs/heads/master`. The import
/// refuses before merging because no `master` was delivered; no raw
/// `NotFound` may escape `import_bundle` -- it must surface as the
/// documented `Sync` contract.
#[test]
fn bundle_import_from_main_branch_repo_reports_sync_not_raw_not_found() {
    let dir1 = ::tempfile::TempDir::new().unwrap();
    let bundle_path = dir1.path().join("main-branch.bundle.tar");
    export_bundle_without_master(&bundle_path, "main");

    // A freshly-initialised repo needs a delivered `master`, which this
    // bundle never provides.
    let (dir2, repo2) = temp_repo();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let db2 = dir2.path().join(".ddb/index.db");
    std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
    let index2 = crate::indexer::Index::open(&db2).unwrap();

    let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
    assert!(
        !matches!(result, Err(DoogatError::NotFound(_))),
        "a merge-engine failure must not escape as a raw NotFound, got {result:?}"
    );
    let err = result.expect_err("importing a bundle with no master branch must fail");
    assert!(
        matches!(err, DoogatError::Sync(_)),
        "the merge-engine failure must surface as Sync, got {err:?}"
    );
    assert!(
        err.to_string().contains("bundle merge failed"),
        "the Sync message must contain the mapping prefix, got: {err}"
    );
}

/// Payload id of a bundle tar, computed from its `objects.bundle` the way
/// the import does.
fn payload_id_of(tar_path: &Path) -> String {
    let dir = ::tempfile::TempDir::new().unwrap();
    tar::Archive::new(std::fs::File::open(tar_path).unwrap())
        .unpack(dir.path())
        .unwrap();
    compute_payload_id(&dir.path().join("objects.bundle")).unwrap()
}

/// Repack `src` into `out` after `edit` changes the extracted files, with a
/// fresh valid v1 checksum.
fn rewrite_bundle(src: &Path, out: &Path, edit: impl FnOnce(&Path)) {
    let dir = ::tempfile::TempDir::new().unwrap();
    tar::Archive::new(std::fs::File::open(src).unwrap())
        .unpack(dir.path())
        .unwrap();
    edit(dir.path());
    let checksum = compute_bundle_checksum(dir.path()).unwrap();
    std::fs::write(dir.path().join("checksum.sha256"), checksum).unwrap();

    let mut builder = tar::Builder::new(std::fs::File::create(out).unwrap());
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if entry.file_type().unwrap().is_dir() {
            builder
                .append_dir_all(name.to_string_lossy().as_ref(), entry.path())
                .unwrap();
        } else {
            builder
                .append_path_with_name(entry.path(), name.to_string_lossy().as_ref())
                .unwrap();
        }
    }
    builder.finish().unwrap();
}

/// Every ref name and direct target (symbolic refs map to `None`), sorted.
fn all_refs(repo_dir: &Path) -> Vec<(String, Option<git2::Oid>)> {
    let repo = git2::Repository::open(repo_dir).unwrap();
    let mut refs: Vec<_> = repo
        .references()
        .unwrap()
        .map(|reference| {
            let reference = reference.expect("ref enumeration must not fail");
            (
                reference
                    .name()
                    .expect("ref name must be UTF-8")
                    .to_string(),
                reference.target(),
            )
        })
        .collect();
    refs.sort();
    refs
}

fn open_index(repo_dir: &Path) -> crate::indexer::Index {
    let db = repo_dir.join(".ddb/index.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    crate::indexer::Index::open(&db).unwrap()
}

/// Import A fails; a legacy flat `bundle/master` ref, a sibling-prefix
/// namespace and an unrelated remote ref exist. A successful multi-branch
/// import C removes exactly its own refs; every other ref/OID survives.
#[test]
fn successful_bundle_import_preserves_other_failed_and_legacy_refs() {
    let (_dir1, dir2, result) = import_with_unresolvable_collision();
    assert!(
        matches!(result, Err(DoogatError::Sync(_))),
        "setup invalid: the collision import A must fail, got {result:?}"
    );
    let repo2 = GitRepo::open(dir2.path()).unwrap();
    let local_head = repo2.repo.head().unwrap().target().unwrap();

    // C descends from node 2 and carries two branches.
    let (dir3, repo3) = cloned_repo(&dir2);
    crate::sync_manager::register_node(&repo3, "Node3").unwrap();
    repo3
        .commit_file(
            "ddb/20260305000000.md",
            "---\nid: 20260305000000\ntitle: C\n---\nC body\n",
            "add c",
        )
        .unwrap();
    let c_head = repo3.repo.head().unwrap().peel_to_commit().unwrap();
    repo3.repo.branch("feature", &c_head, false).unwrap();
    let mgr3 = SyncManager::open(&repo3).unwrap();
    let c_bundle = dir3.path().join("c.bundle.tar");
    export_full_bundle(&repo3, &mgr3, &c_bundle).unwrap();
    let c_ns = format!("refs/remotes/bundle/{}/", payload_id_of(&c_bundle));

    repo2
        .repo
        .reference("refs/remotes/bundle/master", local_head, false, "legacy")
        .unwrap();
    let sibling = format!("{}-sibling/master", c_ns.trim_end_matches('/'));
    repo2
        .repo
        .reference(&sibling, local_head, false, "sibling prefix")
        .unwrap();
    repo2
        .repo
        .reference(
            "refs/remotes/elsewhere/feature",
            local_head,
            false,
            "unrelated",
        )
        .unwrap();

    let others = |refs: Vec<(String, Option<git2::Oid>)>| -> Vec<_> {
        refs.into_iter()
            .filter(|(name, _)| !name.starts_with("refs/heads/") && !name.starts_with(&c_ns))
            .collect()
    };
    let before = others(all_refs(dir2.path()));
    assert!(
        before.iter().any(|(name, _)| name == &sibling),
        "setup invalid: the sibling ref must be outside C's namespace"
    );

    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = open_index(dir2.path());
    import_bundle(&repo2, &mut mgr2, &index2, &c_bundle).expect("C must import cleanly");
    assert!(repo2
        .read_file("ddb/20260305000000.md")
        .unwrap()
        .contains("C body"));

    let after = all_refs(dir2.path());
    assert!(
        after.iter().all(|(name, _)| !name.starts_with(&c_ns)),
        "every ref C fetched must be removed, got {after:?}"
    );
    assert_eq!(
        others(after),
        before,
        "every ref outside C's namespace must survive exactly"
    );
}

/// The payload id is the full SHA-256 of the `objects.bundle` bytes: same
/// bytes, same id; different Git payload, different id. A metadata-only
/// tar change keeps the id yet still runs node registration on retry.
#[test]
fn bundle_payload_identity_is_stable_and_changes_with_git_payload() {
    let dir = ::tempfile::TempDir::new().unwrap();
    let one = dir.path().join("one.bundle");
    let copy = dir.path().join("copy.bundle");
    let two = dir.path().join("two.bundle");
    std::fs::write(&one, b"payload one").unwrap();
    std::fs::write(&copy, b"payload one").unwrap();
    std::fs::write(&two, b"payload two").unwrap();

    let id = compute_payload_id(&one).unwrap();
    assert_eq!(id, format!("{:x}", Sha256::digest(b"payload one")));
    assert_eq!(id.len(), 64, "the id must be the full digest, got {id}");
    assert!(id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    assert_eq!(compute_payload_id(&copy).unwrap(), id);
    assert_ne!(compute_payload_id(&two).unwrap(), id);

    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: test\n---\nBody\n",
            "add",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let first = dir1.path().join("first.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &first).unwrap();
    let second = dir1.path().join("second.bundle.tar");
    rewrite_bundle(&first, &second, |d| {
        std::fs::create_dir_all(d.join("nodes")).unwrap();
        std::fs::write(
            d.join("nodes/extra-node.toml"),
            "uuid = \"extra-node\"\nname = \"Extra\"\nknown_heads = []\nstatus = \"Active\"\n",
        )
        .unwrap();
        let manifest = d.join("manifest.toml");
        let mut text = std::fs::read_to_string(&manifest).unwrap();
        text.push_str("\n# metadata-only change\n");
        std::fs::write(manifest, text).unwrap();
    });
    assert_eq!(payload_id_of(&first), payload_id_of(&second));

    let (dir2, repo2) = temp_repo();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = open_index(dir2.path());
    import_bundle(&repo2, &mut mgr2, &index2, &first).unwrap();
    let extra = dir2.path().join(".nodes/extra-node.toml");
    assert!(
        !extra.exists(),
        "setup invalid: the first tar has no extra node"
    );

    import_bundle(&repo2, &mut mgr2, &index2, &second).expect("a same-payload retry must succeed");
    assert!(
        extra.exists(),
        "a metadata-only change must not skip node registration on retry"
    );
}

/// A valid v1 archive (checksum and manifest verify) without
/// `objects.bundle` fails before any HEAD or ref write.
#[test]
fn bundle_import_rejects_missing_git_payload_before_repo_mutation() {
    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: test\n---\nBody\n",
            "add",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let full = dir1.path().join("full.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &full).unwrap();
    let stripped = dir1.path().join("stripped.bundle.tar");
    rewrite_bundle(&full, &stripped, |d| {
        std::fs::remove_file(d.join("objects.bundle")).unwrap();
    });
    verify_bundle(&stripped).expect("setup invalid: the stripped archive must verify as v1");

    let (dir2, repo2) = temp_repo();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let head_before = repo2.head_oid().unwrap().0;
    let refs_before = all_refs(dir2.path());
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = open_index(dir2.path());

    let err = import_bundle(&repo2, &mut mgr2, &index2, &stripped)
        .expect_err("a bundle without objects.bundle must fail");
    assert!(
        matches!(err, DoogatError::Validation(_)) && err.to_string().contains("objects.bundle"),
        "the missing Git payload must be named, got {err:?}"
    );
    assert_eq!(repo2.head_oid().unwrap().0, head_before);
    assert_eq!(all_refs(dir2.path()), refs_before, "no ref may be written");
}

/// Refs already in the candidate namespace must match the advertised heads.
/// A wrong OID on an advertised branch, or a `master` the bundle does not
/// advertise, is refused loudly: no force, no merge, no cleanup.
#[test]
fn bundle_import_rejects_unexpected_existing_namespace_refs() {
    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: Remote\n---\nRemote body\n",
            "add",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let with_master = dir1.path().join("master.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &with_master).unwrap();

    let (dir2, repo2) = temp_repo();
    repo2
        .commit_file(
            "ddb/20260310000000.md",
            "---\nid: 20260310000000\ntitle: Local\n---\nLocal body\n",
            "add local",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let local_head = repo2.repo.head().unwrap().target().unwrap();
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = open_index(dir2.path());

    // Wrong OID on the advertised `master`.
    let ns = format!("refs/remotes/bundle/{}/", payload_id_of(&with_master));
    repo2
        .repo
        .reference(&format!("{ns}master"), local_head, false, "seed wrong oid")
        .unwrap();
    let err = import_bundle(&repo2, &mut mgr2, &index2, &with_master)
        .expect_err("a wrong-OID namespace ref must be refused");
    assert!(
        matches!(err, DoogatError::Sync(_)) && err.to_string().contains(&ns),
        "the refusal must name the namespace, got {err:?}"
    );
    assert_eq!(
        bundle_refs(dir2.path()),
        vec![(format!("{ns}master"), local_head)],
        "nothing may be fetched, forced or deleted"
    );
    assert_eq!(repo2.head_oid().unwrap().0, local_head.to_string());
    assert!(repo2.read_file("ddb/20260301000000.md").is_err());

    // A masterless bundle whose namespace already holds a `master`: the
    // advertised heads decide, so the seeded ref is refused, never merged.
    let masterless = dir2.path().join("masterless.bundle.tar");
    export_bundle_without_master(&masterless, "sidecar");
    let ns_b = format!("refs/remotes/bundle/{}/", payload_id_of(&masterless));
    repo2
        .repo
        .reference(&format!("{ns_b}master"), local_head, false, "seed extra")
        .unwrap();
    let err = import_bundle(&repo2, &mut mgr2, &index2, &masterless)
        .expect_err("an unadvertised namespace ref must be refused");
    assert!(
        matches!(err, DoogatError::Sync(_)) && err.to_string().contains(&ns_b),
        "the refusal must name the namespace, got {err:?}"
    );
    let in_b: Vec<_> = bundle_refs(dir2.path())
        .into_iter()
        .filter(|(name, _)| name.starts_with(&ns_b))
        .collect();
    assert_eq!(in_b, vec![(format!("{ns_b}master"), local_head)]);
    assert_eq!(repo2.head_oid().unwrap().0, local_head.to_string());
    assert!(repo2.read_file("ddb/20260401000000.md").is_err());
}

/// A failed import's retry, after the conflict is repaired, reuses the
/// same payload namespace, clears only it, keeps sibling refs, and releases
/// the import lease. A repeated success is a no-op.
#[test]
fn failed_bundle_retry_uses_same_namespace_and_releases_import_lease() {
    let (dir1, dir2, result) = import_with_unresolvable_collision();
    assert!(
        matches!(result, Err(DoogatError::Sync(_))),
        "setup invalid: the collision import A must fail, got {result:?}"
    );
    let a_bundle = dir1.path().join("collision.bundle.tar");
    let ns = format!("refs/remotes/bundle/{}/", payload_id_of(&a_bundle));
    let failed = bundle_refs(dir2.path());
    assert!(
        !failed.is_empty() && failed.iter().all(|(name, _)| name.starts_with(&ns)),
        "A's refs must live in its payload namespace, got {failed:?}"
    );
    let git_dir = dir2.path().join(".git");
    drop(
        write_lock::acquire(
            &git_dir,
            "ddb-bundle-import.lock",
            Duration::from_millis(200),
        )
        .expect("a failed import must release the import lease"),
    );

    let repo2 = GitRepo::open(dir2.path()).unwrap();
    let kept_oid = failed[0].1;
    let siblings = [
        "refs/remotes/bundle/other/master".to_string(),
        format!("{}x/master", ns.trim_end_matches('/')),
    ];
    for name in &siblings {
        repo2
            .repo
            .reference(name, kept_oid, false, "sibling")
            .unwrap();
    }

    // Repair the collision locally with A's exact content.
    repo2
        .commit_file(
            "ddb/20260302000000.md",
            UNREWRITABLE_LOSER,
            "repair collision locally",
        )
        .unwrap();

    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = open_index(dir2.path());
    import_bundle(&repo2, &mut mgr2, &index2, &a_bundle).expect("the repaired retry must succeed");

    let after = bundle_refs(dir2.path());
    assert!(
        after.iter().all(|(name, _)| !name.starts_with(&ns)),
        "the retry must clear its namespace, got {after:?}"
    );
    for name in &siblings {
        assert!(
            after.contains(&(name.clone(), kept_oid)),
            "sibling {name} must survive, got {after:?}"
        );
    }
    drop(
        write_lock::acquire(
            &git_dir,
            "ddb-bundle-import.lock",
            Duration::from_millis(200),
        )
        .expect("a successful import must release the import lease"),
    );

    let head = repo2.head_oid().unwrap().0;
    import_bundle(&repo2, &mut mgr2, &index2, &a_bundle).expect("a repeated success must succeed");
    assert_eq!(
        repo2.head_oid().unwrap().0,
        head,
        "a repeated success is a no-op"
    );
    assert_eq!(bundle_refs(dir2.path()), after);
}

/// Child-only env (set on the spawned command, never globally) selecting the
/// lease test's child mode and fixture.
const LEASE_CHILD_MODE: &str = "DDB_TEST_BUNDLE_LEASE_CHILD";
const LEASE_CHILD_REPO: &str = "DDB_TEST_BUNDLE_LEASE_REPO";
const LEASE_CHILD_BUNDLE: &str = "DDB_TEST_BUNDLE_LEASE_BUNDLE";

/// Child side of the lease test: one real import on independent handles in
/// its own process. Prints a marker only after every assertion held.
fn run_lease_child(mode: &str) {
    let repo_dir = PathBuf::from(std::env::var(LEASE_CHILD_REPO).unwrap());
    let bundle = PathBuf::from(std::env::var(LEASE_CHILD_BUNDLE).unwrap());
    let repo = GitRepo::open(&repo_dir).unwrap();
    let mut mgr = SyncManager::open(&repo).unwrap();
    let index = open_index(&repo_dir);
    match mode {
        "refuse" => {
            let start = std::time::Instant::now();
            let err = import_bundle_with_lease_timeout(
                &repo,
                &mut mgr,
                &index,
                &bundle,
                Duration::from_millis(200),
            )
            .expect_err("a held lease must refuse after the bounded wait");
            assert!(
                matches!(err, DoogatError::Conflict(_))
                    && err.to_string().contains("bundle-import"),
                "lease contention must be a Conflict naming bundle import, got {err:?}"
            );
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the wait must be bounded"
            );
        }
        "import" => {
            let report = import_bundle_with_lease_timeout(
                &repo,
                &mut mgr,
                &index,
                &bundle,
                Duration::from_secs(20),
            )
            .expect("the import must succeed once the lease is free");
            assert_eq!(report.direction, "bundle-import");
        }
        other => panic!("unknown lease child mode {other:?}"),
    }
    println!("lease-child-done:{mode}");
}

/// Re-run this exact test in a fresh process of this test binary as child
/// `mode`, wait for it (bounded, kill and reap on deadline), and panic with
/// its output unless it exited cleanly after printing its marker.
fn spawn_lease_child(mode: &str, repo_dir: &Path, bundle: &Path) {
    let test_name = concat!(
        module_path!(),
        "::bundle_import_lease_covers_fetch_through_cleanup"
    )
    .split_once("::")
    .unwrap()
    .1;
    let logs = ::tempfile::TempDir::new().unwrap();
    let out_path = logs.path().join("stdout");
    let err_path = logs.path().join("stderr");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([test_name, "--exact", "--nocapture", "--test-threads=1"])
        .env(LEASE_CHILD_MODE, mode)
        .env(LEASE_CHILD_REPO, repo_dir)
        .env(LEASE_CHILD_BUNDLE, bundle)
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = format!(
        "stdout:\n{}\nstderr:\n{}",
        std::fs::read_to_string(&out_path).unwrap_or_default(),
        std::fs::read_to_string(&err_path).unwrap_or_default()
    );
    assert!(
        status.is_some_and(|s| s.success())
            && output.contains(&format!("lease-child-done:{mode}")),
        "lease child {mode:?} did not complete (status {status:?}, None = killed at deadline):\n{output}"
    );
}

/// The import lease covers fetch through cleanup across processes: while
/// this process holds it, a child process's real import refuses loudly after
/// its bounded wait and leaves every ref and HEAD untouched. The lease stays
/// held until the child has finished, so the refusal proves the import tried
/// to enter while held. Once released, a second child imports and clears only
/// its namespace; an error path releases the lease too.
#[test]
fn bundle_import_lease_covers_fetch_through_cleanup() {
    if let Ok(mode) = std::env::var(LEASE_CHILD_MODE) {
        run_lease_child(&mode);
        return;
    }
    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: Base\n---\nBase body\n",
            "add base",
        )
        .unwrap();
    let base_oid = repo1.repo.head().unwrap().target().unwrap();
    // Node 2 already holds the base commit, which the bundle's `feature` names.
    let (dir2, repo2) = cloned_repo(&dir1);
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    repo1
        .repo
        .branch("feature", &repo1.repo.find_commit(base_oid).unwrap(), false)
        .unwrap();
    repo1
        .commit_file(
            "ddb/20260306000000.md",
            "---\nid: 20260306000000\ntitle: New\n---\nNew body\n",
            "add new",
        )
        .unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let bundle = dir1.path().join("lease.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle).unwrap();
    let ns = format!("refs/remotes/bundle/{}/", payload_id_of(&bundle));

    // Another importer owns the lease and has fetched `feature` already.
    let git_dir = dir2.path().join(".git");
    let held =
        write_lock::acquire(&git_dir, "ddb-bundle-import.lock", Duration::from_secs(5)).unwrap();
    repo2
        .repo
        .reference(&format!("{ns}feature"), base_oid, false, "holder fetched")
        .unwrap();
    let seeded = bundle_refs(dir2.path());
    let head_before = repo2.head_oid().unwrap().0;

    // A child process's real import refuses while this process holds the
    // lease; the lease is dropped only after the child has finished.
    spawn_lease_child("refuse", dir2.path(), &bundle);
    assert_eq!(
        bundle_refs(dir2.path()),
        seeded,
        "a refused import must not fetch or clean"
    );
    assert_eq!(repo2.head_oid().unwrap().0, head_before);
    drop(held);

    let sibling = "refs/remotes/bundle/other/master".to_string();
    repo2
        .repo
        .reference(&sibling, base_oid, false, "sibling")
        .unwrap();
    spawn_lease_child("import", dir2.path(), &bundle);
    let reopened = GitRepo::open(dir2.path()).unwrap();
    assert!(reopened.read_file("ddb/20260306000000.md").is_ok());
    assert_eq!(
        bundle_refs(dir2.path()),
        vec![(sibling, base_oid)],
        "the import must clear exactly its own namespace"
    );

    // Error path releases the lease too.
    let masterless = dir2.path().join("masterless.bundle.tar");
    export_bundle_without_master(&masterless, "sidecar");
    let mut mgr = SyncManager::open(&reopened).unwrap();
    let index = open_index(dir2.path());
    assert!(import_bundle(&reopened, &mut mgr, &index, &masterless).is_err());
    drop(
        write_lock::acquire(
            &git_dir,
            "ddb-bundle-import.lock",
            Duration::from_millis(200),
        )
        .expect("a failed import must release the import lease"),
    );
}

/// A real import keeps the import lease between its fetch and its cleanup.
/// This test holds the Git write lock the merge needs, so the importer
/// thread blocks at the merge once its fetch has landed. While it is blocked
/// the lease cannot be taken; once the write lock is released the import
/// completes and clears its namespace, and only then is the lease free.
#[test]
fn real_import_holds_import_lease_while_blocked_at_merge() {
    let (dir2, repo2) = temp_repo();
    repo2
        .commit_file(
            "ddb/20260301000000.md",
            "---\nid: 20260301000000\ntitle: Base\n---\nBase body\n",
            "add base",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let (dir1, repo1) = cloned_repo(&dir2);
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    repo1
        .commit_file(
            "ddb/20260307000000.md",
            "---\nid: 20260307000000\ntitle: Held\n---\nHeld body\n",
            "add held",
        )
        .unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let bundle = dir1.path().join("held.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle).unwrap();
    let bundle_head = repo1.repo.head().unwrap().target().unwrap();
    let fetched_master = (
        format!("refs/remotes/bundle/{}/master", payload_id_of(&bundle)),
        bundle_head,
    );
    let head_before = repo2.head_oid().unwrap().0;
    let git_dir = dir2.path().join(".git");

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let repo_dir = dir2.path().to_path_buf();
    let importer = std::thread::spawn(move || {
        let repo = GitRepo::open(&repo_dir).unwrap();
        let mut mgr = SyncManager::open(&repo).unwrap();
        let index = open_index(&repo_dir);
        ready_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        import_bundle(&repo, &mut mgr, &index, &bundle)
            .map(|report| report.direction)
            .map_err(|e| e.to_string())
    });
    ready_rx.recv().expect("the importer must open its handles");
    let write_guard =
        write_lock::acquire(&git_dir, "ddb-write.lock", Duration::from_secs(5)).unwrap();
    go_tx.send(()).unwrap();

    // Rendezvous on observable state: the fetched ref appears only after the
    // importer took the lease and fetched, and its merge cannot run while
    // this test holds the write lock (the merge waits up to 10 s for it).
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !bundle_refs(dir2.path()).contains(&fetched_master) {
        assert!(
            !importer.is_finished(),
            "the importer finished before its fetch was observed"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the importer never fetched {fetched_master:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let contended = write_lock::acquire(
        &git_dir,
        "ddb-bundle-import.lock",
        Duration::from_millis(200),
    );
    assert!(
        matches!(contended, Err(DoogatError::Conflict(_))),
        "a real import blocked between fetch and merge must still hold the import lease"
    );
    assert_eq!(
        GitRepo::open(dir2.path()).unwrap().head_oid().unwrap().0,
        head_before,
        "setup invalid: the merge must still be blocked on the write lock"
    );

    drop(write_guard);
    let outcome = importer.join().expect("the importer thread must not panic");
    assert_eq!(outcome, Ok("bundle-import".to_string()));
    assert_eq!(
        GitRepo::open(dir2.path()).unwrap().head_oid().unwrap().0,
        bundle_head.to_string(),
        "the released import must land"
    );
    assert!(
        bundle_refs(dir2.path()).is_empty(),
        "the released import must clean its namespace"
    );
    drop(
        write_lock::acquire(
            &git_dir,
            "ddb-bundle-import.lock",
            Duration::from_millis(200),
        )
        .expect("a finished import must release the import lease"),
    );
}

/// `parse_ref_lines` never skips a malformed record: a short or uppercase
/// OID, a missing separator, an empty name, a name with whitespace and a
/// blank line in the middle each fail as `Sync` naming the record. Full
/// 40- and 64-hex OIDs parse into `(refname, oid)` pairs.
#[test]
fn parse_ref_lines_rejects_malformed_records_and_accepts_full_oids() {
    let sha1 = "a".repeat(40);
    let sha256 = "0123456789abcdef".repeat(4);
    let parsed = parse_ref_lines(
        &format!("{sha1} refs/heads/master\n{sha256} refs/heads/feature\n"),
        "test",
    )
    .unwrap();
    assert_eq!(
        parsed,
        vec![
            ("refs/heads/master".to_string(), sha1.clone()),
            ("refs/heads/feature".to_string(), sha256.clone()),
        ]
    );

    let malformed = [
        format!("{} refs/heads/master", &sha1[..39]),
        format!("{} refs/heads/master", sha1.to_uppercase()),
        format!("{sha1}refs/heads/master"),
        format!("{sha1} "),
        format!("{sha1} refs/heads/a b"),
        format!("{sha1} refs/heads/a\tb"),
        String::new(),
    ];
    for line in &malformed {
        let text = format!("{sha1} refs/heads/before\n{line}\n{sha256} refs/heads/after\n");
        match parse_ref_lines(&text, "test") {
            Err(DoogatError::Sync(msg)) => assert!(
                msg.contains(&format!("malformed test record: {line:?}")),
                "the error must name the record {line:?}, got: {msg}"
            ),
            other => panic!("record {line:?} must be refused as Sync, got {other:?}"),
        }
    }
}

/// Tags in a bundle are never fetched: a lightweight and an annotated tag
/// pointing into the bundle's history must not land in `refs/tags/*`
/// outside the payload namespace, and never select a branch.
#[test]
fn bundle_import_never_fetches_advertised_tags() {
    let (dir1, repo1) = temp_repo();
    repo1
        .commit_file(
            "ddb/20260308000000.md",
            "---\nid: 20260308000000\ntitle: Tagged\n---\nTagged body\n",
            "add tagged",
        )
        .unwrap();
    crate::sync_manager::register_node(&repo1, "Node1").unwrap();
    let head = repo1.repo.head().unwrap().peel_to_commit().unwrap();
    repo1
        .repo
        .tag_lightweight("light", head.as_object(), false)
        .unwrap();
    let sig = git2::Signature::now("ddb", "ddb@example.invalid").unwrap();
    repo1
        .repo
        .tag("annotated", head.as_object(), &sig, "annotated tag", false)
        .unwrap();
    let mgr1 = SyncManager::open(&repo1).unwrap();
    let bundle = dir1.path().join("tagged.bundle.tar");
    export_full_bundle(&repo1, &mgr1, &bundle).unwrap();

    let extracted = ::tempfile::TempDir::new().unwrap();
    tar::Archive::new(std::fs::File::open(&bundle).unwrap())
        .unpack(extracted.path())
        .unwrap();
    let git_bundle = extracted.path().join("objects.bundle");
    let listed = run_git(
        &repo1,
        &["bundle", "list-heads", git_bundle.to_str().unwrap()],
    )
    .unwrap();
    assert!(
        listed.contains("refs/tags/light") && listed.contains("refs/tags/annotated"),
        "setup invalid: the bundle must advertise both tags, got {listed}"
    );
    assert_eq!(
        advertised_heads(&repo1, &git_bundle).unwrap(),
        vec![("master".to_string(), head.id().to_string())],
        "advertised tags and HEAD must never select a branch"
    );

    let (dir2, repo2) = temp_repo();
    crate::sync_manager::register_node(&repo2, "Node2").unwrap();
    let tags = |dir: &Path| -> Vec<(String, Option<git2::Oid>)> {
        all_refs(dir)
            .into_iter()
            .filter(|(name, _)| name.starts_with("refs/tags/"))
            .collect()
    };
    let tags_before = tags(dir2.path());
    let mut mgr2 = SyncManager::open(&repo2).unwrap();
    let index2 = open_index(dir2.path());
    import_bundle(&repo2, &mut mgr2, &index2, &bundle).expect("a tagged bundle must import");
    assert!(repo2
        .read_file("ddb/20260308000000.md")
        .unwrap()
        .contains("Tagged body"));
    assert_eq!(
        tags(dir2.path()),
        tags_before,
        "advertised tags must never be fetched into refs/tags/*"
    );
}
