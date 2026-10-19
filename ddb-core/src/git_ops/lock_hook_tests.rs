use super::GitRepo;
use crate::traits::GitBackend;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc;
use std::time::Duration;

/// Session commit counter as `check_write_threshold` left it. With the
/// default config (auto-maintenance disabled) the check only counts, so
/// this equals the number of commits the check has seen since the last
/// reset — the pre-FT-5 inline behavior the deferral must preserve.
fn counted(repo: &GitRepo) -> u32 {
    repo.session_commits.load(Relaxed)
}

/// On-disk commit-graph bytes, the black-box oracle for "did the
/// `git commit-graph write` subprocess run": every reachable commit is
/// listed, so a new commit changes the file.
fn commit_graph_bytes(repo: &GitRepo) -> Option<Vec<u8>> {
    std::fs::read(repo.path.join(".git/objects/info/commit-graph")).ok()
}

#[test]
fn hook_is_reentrant_for_commit_paths_on_the_same_instance() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    let commit = GitBackend::with_write_lock(&repo, || {
        assert_eq!(repo.write_lock_depth.get(), 1);
        repo.commit_file("ddb/x.md", "inside", "nested commit")
    })
    .unwrap();
    assert_eq!(repo.head_oid().unwrap(), commit);
    assert_eq!(repo.read_file("ddb/x.md").unwrap(), "inside");
    assert_eq!(repo.write_lock_depth.get(), 0);
}

#[test]
fn hook_excludes_a_second_handle_until_release() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    let path = dir.path().to_path_buf();
    let (tx, rx) = mpsc::channel();

    GitBackend::with_write_lock(&repo, || {
        std::thread::spawn(move || {
            let other = GitRepo::open(&path).unwrap();
            let r = other.commit_file("ddb/y.md", "other", "second handle");
            tx.send(r.map(|_| ())).unwrap();
        });
        // The second handle must still be blocked while we hold the lock.
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "second GitRepo handle committed while the hook held the lock"
        );
        Ok(())
    })
    .unwrap();

    rx.recv_timeout(Duration::from_secs(15))
        .expect("second handle never finished after release")
        .expect("second handle's commit failed");
    assert_eq!(repo.read_file("ddb/y.md").unwrap(), "other");
}

/// Two commits nested in one lock must each advance the session counter
/// once after release (a single pending flag collapsed them to one), and
/// the counter must not move while the lock is held.
#[test]
fn every_nested_commit_counts_toward_maintenance_after_release() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.reset_session_commits();
    assert_eq!(counted(&repo), 0);

    GitBackend::with_write_lock(&repo, || {
        repo.commit_file("ddb/a.md", "a", "first nested")?;
        GitBackend::with_write_lock(&repo, || {
            repo.commit_file("ddb/b.md", "b", "second nested")
        })?;
        assert_eq!(repo.uncounted_commits.get(), 2);
        assert_eq!(
            counted(&repo),
            0,
            "check_write_threshold ran under the lock"
        );
        Ok(())
    })
    .unwrap();

    assert_eq!(repo.uncounted_commits.get(), 0);
    assert_eq!(counted(&repo), 2, "two commits must count twice, not once");

    // Sanity: an unnested commit still counts exactly once, as before FT-5.
    repo.commit_file("ddb/c.md", "c", "plain").unwrap();
    assert_eq!(counted(&repo), 3);
}

/// With maintenance enabled the deferred checks still cross the threshold
/// exactly as the inline ones did: two nested commits under threshold 3
/// leave the counter at 2 (not 1, not reset); the third commit fires it.
#[test]
fn deferred_checks_cross_the_write_threshold_like_inline_ones() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file(
        ".ddb.toml",
        "[maintenance]\nauto_enabled = true\nwrite_threshold = 3\n",
        "enable maintenance",
    )
    .unwrap();
    repo.reset_session_commits();

    GitBackend::with_write_lock(&repo, || {
        repo.commit_file("ddb/a.md", "a", "first nested")?;
        repo.commit_file("ddb/b.md", "b", "second nested")
    })
    .unwrap();
    assert_eq!(
        counted(&repo),
        2,
        "below threshold: both commits counted, none reset"
    );

    repo.commit_file("ddb/c.md", "c", "third").unwrap();
    assert_eq!(
        counted(&repo),
        0,
        "threshold reached: maintenance ran and reset"
    );
}

/// A nested commit must leave the on-disk commit-graph untouched until the
/// outermost lock is released, then the deferred rewrite must include it.
#[test]
fn commit_graph_is_rewritten_only_after_the_outermost_lock_is_released() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file("ddb/seed.md", "seed", "seed").unwrap();
    let before = commit_graph_bytes(&repo)
        .expect("git commit-graph write must produce .git/objects/info/commit-graph");

    GitBackend::with_write_lock(&repo, || {
        GitBackend::with_write_lock(&repo, || repo.commit_file("ddb/g.md", "g", "nested"))?;
        assert!(repo.commit_graph_pending.get());
        assert_eq!(
            commit_graph_bytes(&repo).as_ref(),
            Some(&before),
            "commit-graph was rewritten under the write lock"
        );
        Ok(())
    })
    .unwrap();

    assert!(!repo.commit_graph_pending.get());
    let after = commit_graph_bytes(&repo).expect("commit-graph still present");
    assert_ne!(
        after, before,
        "deferred commit-graph rewrite did not run after release"
    );
}

/// `set_skip_commit_graph(true)` (sync's batch mode) must suppress the
/// deferred rewrite too, and the explicit post-batch `write_commit_graph`
/// outside any lock must still run.
#[test]
fn deferred_commit_graph_rewrite_honors_skip_commit_graph() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file("ddb/seed.md", "seed", "seed").unwrap();
    let before = commit_graph_bytes(&repo).expect("seed commit wrote the commit-graph");

    repo.set_skip_commit_graph(true);
    GitBackend::with_write_lock(&repo, || repo.commit_file("ddb/s.md", "s", "skipped"))
        .unwrap();
    assert!(!repo.commit_graph_pending.get());
    assert_eq!(
        commit_graph_bytes(&repo).as_ref(),
        Some(&before),
        "commit-graph was rewritten despite skip_commit_graph"
    );

    repo.set_skip_commit_graph(false);
    repo.write_commit_graph();
    assert_ne!(
        commit_graph_bytes(&repo).expect("commit-graph still present"),
        before,
        "explicit write_commit_graph outside the lock did not run"
    );
}

/// An `Err` after a durable commit still runs both deferred subprocess
/// steps after release, propagates the error, and leaves no lock state.
#[test]
fn deferred_work_still_runs_when_the_locked_body_fails_after_committing() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    repo.commit_file("ddb/seed.md", "seed", "seed").unwrap();
    let before = commit_graph_bytes(&repo).expect("seed commit wrote the commit-graph");
    repo.reset_session_commits();

    let err = GitBackend::with_write_lock(&repo, || {
        repo.commit_file("ddb/w.md", "w", "committed then failed")?;
        Err::<(), _>(crate::error::DoogatError::Validation("after commit".into()))
    })
    .unwrap_err();
    assert!(matches!(err, crate::error::DoogatError::Validation(_)));
    assert_eq!(repo.write_lock_depth.get(), 0);
    assert_eq!(repo.uncounted_commits.get(), 0);
    assert!(!repo.commit_graph_pending.get());
    // The commit landed, so the deferred check counted it and the graph
    // was rewritten to include it.
    assert_eq!(repo.read_file("ddb/w.md").unwrap(), "w");
    assert_eq!(counted(&repo), 1);
    assert_ne!(
        commit_graph_bytes(&repo).expect("commit-graph still present"),
        before
    );
}
