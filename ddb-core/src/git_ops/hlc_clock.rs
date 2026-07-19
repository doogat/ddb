// Machine-local, monotonic HLC clock persisted to `<git_dir>/ddb-hlc`.
//
// Holds the `pub(crate) struct HlcClock` (`load` / `tick` / `recv`) that stamps
// every write-commit trailer from a machine-local monotonic clock, plus its
// behavior tests below.

use std::path::PathBuf;

use crate::hlc::Hlc;

/// Larger of two optional HLCs (`None` if both are `None`).
fn max_opt(a: Option<Hlc>, b: Option<Hlc>) -> Option<Hlc> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Machine-local, monotonic Hybrid Logical Clock cached in `<git_dir>/ddb-hlc`.
///
/// The durable source of truth for causality is git (HLC commit trailers); the
/// `ddb-hlc` file is a best-effort cache that keeps short-lived processes and
/// two clocks over the same repo monotonic without walking history each tick.
pub(crate) struct HlcClock {
    state_path: PathBuf,
    node_id: String,
    floor: std::cell::RefCell<Option<Hlc>>,
}

impl HlcClock {
    /// Load the clock, seeding from the on-disk cache and/or the HEAD trailer.
    ///
    /// Never panics. The node id comes from `<git_dir>/ddb-node` when present,
    /// else a fresh ephemeral uuid (the file is neither created nor written).
    /// The recovery seed is the larger of the parsed `ddb-hlc` line and HEAD's
    /// HLC trailer; when present it is written back to repair the cache.
    pub(crate) fn load(repo: &git2::Repository) -> Self {
        let git_dir = repo.path();
        let state_path = git_dir.join("ddb-hlc");

        let node_id = std::fs::read_to_string(git_dir.join("ddb-node"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let from_file = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|s| Hlc::parse(s.trim()).ok());
        let from_head = match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
            Some(commit) => crate::hlc::extract_hlc(commit.message().unwrap_or("")),
            None => None,
        };
        let seed = max_opt(from_file, from_head);

        let clock = Self {
            state_path,
            node_id,
            floor: std::cell::RefCell::new(seed.clone()),
        };
        if let Some(h) = &seed {
            clock.persist(h);
        }
        clock
    }

    /// Tick for a local event: advance past the on-disk and in-memory floor.
    pub(crate) fn tick(&self) -> Hlc {
        let base = max_opt(self.read_persisted(), self.floor.borrow().clone());
        let h = Hlc::now(&self.node_id, &base);
        *self.floor.borrow_mut() = Some(h.clone());
        self.persist(&h);
        h
    }

    /// Merge a remote clock in, folding it against the accumulated local floor.
    pub(crate) fn recv(&self, remote: &Hlc) -> Hlc {
        let base = max_opt(self.read_persisted(), self.floor.borrow().clone());
        let h = Hlc::recv(&self.node_id, &base, remote);
        *self.floor.borrow_mut() = Some(h.clone());
        self.persist(&h);
        h
    }

    /// Read and parse the persisted `ddb-hlc` line; `None` if missing/corrupt.
    fn read_persisted(&self) -> Option<Hlc> {
        std::fs::read_to_string(&self.state_path)
            .ok()
            .and_then(|s| Hlc::parse(s.trim()).ok())
    }

    /// Best-effort atomic persist (temp file + rename); warns, never fails.
    fn persist(&self, h: &Hlc) {
        let tmp = self.state_path.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, h.to_string()) {
            tracing::warn!("failed to write ddb-hlc temp file: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &self.state_path) {
            tracing::warn!("failed to persist ddb-hlc: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::hlc::Hlc;
    use tempfile::TempDir;

    /// A single clock's ticks are strictly increasing (by `Hlc` Ord) across many
    /// sequential calls. Rejects any impl whose ticks can repeat or go backwards.
    #[test]
    fn tick_is_strictly_monotonic() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let clock = super::HlcClock::load(&repo);

        let mut last: Option<Hlc> = None;
        for _ in 0..100 {
            let next = clock.tick();
            if let Some(prev) = &last {
                assert!(
                    next > *prev,
                    "tick must be strictly monotonic: {prev} >= {next}"
                );
            }
            last = Some(next);
        }
    }

    /// Two separate `HlcClock` instances over the same repo share one `ddb-hlc`
    /// file; interleaved ticks stay strictly monotonic. This only holds if each
    /// tick re-reads the persisted state rather than trusting a stale in-memory
    /// copy captured at `load`.
    #[test]
    fn two_clocks_sharing_state_file_stay_monotonic() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        let clock_a = super::HlcClock::load(&repo);
        let clock_b = super::HlcClock::load(&repo);

        let mut last: Option<Hlc> = None;
        for i in 0..50 {
            let next = if i % 2 == 0 {
                clock_a.tick()
            } else {
                clock_b.tick()
            };
            if let Some(prev) = &last {
                assert!(
                    next > *prev,
                    "interleaved ticks across two clocks must be strictly monotonic: {prev} >= {next}"
                );
            }
            last = Some(next);
        }
    }

    /// `recv` absorbs a far-future remote clock (its wall time jumps to at least
    /// the remote's), and the absorbed value is persisted: a later `tick` reads
    /// it back and advances past it instead of regressing to bare wall-clock time.
    #[test]
    fn recv_absorbs_far_future_remote_and_persists() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let clock = super::HlcClock::load(&repo);

        let remote = Hlc {
            wall_ms: u64::MAX / 2,
            counter: 7,
            node: "remote00".into(),
        };
        let received = clock.recv(&remote);
        assert!(
            received.wall_ms >= remote.wall_ms,
            "recv must absorb a far-future remote clock: {} < {}",
            received.wall_ms,
            remote.wall_ms
        );

        // recv must persist: a fresh tick reads the absorbed value and advances
        // past it. A recv that did not persist would tick from ~now (<< remote)
        // and fail this.
        let next = clock.tick();
        assert!(
            next > received,
            "tick after recv must advance past the absorbed remote: {received} >= {next}"
        );
    }

    /// `recv` folds in ACCUMULATED LOCAL state, not just `max(remote, wall-now)`.
    /// Local state is pushed far-future and advanced by a tick before a LOW remote
    /// arrives; the result must not regress below that local high-water mark. An
    /// impl that discards local history (passing `&None` for local state) computes
    /// ~wall-clock-now for the low remote and fails.
    #[test]
    fn recv_folds_in_local_state_not_just_remote() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let clock = super::HlcClock::load(&repo);

        // 1) Push local state far-future via a far-future remote.
        let far_future = Hlc {
            wall_ms: u64::MAX / 2,
            counter: 7,
            node: "remote00".into(),
        };
        let _ = clock.recv(&far_future);

        // 2) Advance once so the persisted local high-water mark is far-future+.
        let local_high = clock.tick();

        // 3) A LOW remote must not drag the clock back to wall-clock time.
        let low = Hlc {
            wall_ms: 1000,
            counter: 0,
            node: "remote01".into(),
        };
        let result = clock.recv(&low);
        assert!(
            result.wall_ms >= local_high.wall_ms,
            "recv must fold in accumulated local state, not discard it for a low remote: {} < {}",
            result.wall_ms,
            local_high.wall_ms
        );
    }

    /// After HEAD carries a far-future HLC trailer, corrupting `ddb-hlc` and
    /// reloading must recover from the committed HLC, not fall back to bare
    /// wall-clock time. The next tick must derive EXACTLY from HEAD's trailer:
    /// same `wall_ms`, `counter` bumped by one. A hardcoded far-future constant
    /// that ignores HEAD's actual counter fails the `== counter + 1` check.
    #[test]
    fn recovers_from_wiped_ddb_hlc_without_regressing() {
        let dir = TempDir::new().unwrap();
        let git_repo = crate::git_ops::GitRepo::init(dir.path()).unwrap();

        // Give HEAD a far-future HLC trailer via a message-only commit that
        // reuses HEAD's tree, so extract_hlc(HEAD.message()) yields `high`.
        // `wall_ms` must stay far-future (else the current wall clock supersedes
        // the seed); `counter` is distinctive (42) so the assertion below can
        // prove the value came FROM HEAD, not from a hardcoded constant.
        let high = Hlc {
            wall_ms: u64::MAX / 2,
            counter: 42,
            node: "seedaaaa".into(),
        };
        let msg = crate::hlc::append_hlc_trailer("seed", &high);
        {
            let raw = &git_repo.repo;
            let sig = git2::Signature::now("ddb", "ddb@local").unwrap();
            let parent = raw.head().unwrap().peel_to_commit().unwrap();
            let tree = parent.tree().unwrap();
            raw.commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&parent])
                .unwrap();
        }

        // Corrupt the machine-local state file.
        let state = git_repo.repo.path().join("ddb-hlc");
        std::fs::write(&state, "not-a-valid-hlc-line\n").unwrap();

        // Recovery must seed from the HEAD trailer, NOT from bare wall time.
        let clock = super::HlcClock::load(&git_repo.repo);
        let next = clock.tick();
        // A correct impl seeds `last = high` then ticks `Hlc::now(node, Some(high))`:
        // the current wall clock is far below `u64::MAX / 2`, so wall_ms is carried
        // through unchanged and the counter bumps 42 -> 43. An impl that returns a
        // hardcoded far-future constant fails these exact-derivation checks.
        assert_eq!(
            next.wall_ms, high.wall_ms,
            "recovery must carry HEAD's wall_ms through, not fall back to wall time: {} != {}",
            next.wall_ms, high.wall_ms
        );
        assert_eq!(
            next.counter,
            high.counter + 1,
            "recovery must read HEAD's counter and bump it once: {} != {}",
            next.counter,
            high.counter + 1
        );
    }

    /// The recovery seed comes strictly from a HEAD HLC trailer, never a baked-in
    /// far-future constant. On a repo whose HEAD has NO trailer, corrupting
    /// `ddb-hlc` and reloading must fall back to the current wall-clock band, not
    /// jump far-future. Paired with the test above, this pins the far-future value
    /// to HEAD: an impl cannot be far-future there AND wall-clock here without
    /// actually reading HEAD.
    #[test]
    fn recovery_falls_back_to_wall_clock_when_head_has_no_trailer() {
        let dir = TempDir::new().unwrap();
        let git_repo = crate::git_ops::GitRepo::init(dir.path()).unwrap();

        // Post-chokepoint, GitRepo::init's own bootstrap commit carries an HLC
        // trailer, so simulate legacy trailer-less history with a raw commit
        // (bypassing create_commit) whose message has no trailer. `load` seeds
        // only from HEAD's own message, so a tip with no trailer IS the
        // "no HEAD trailer" case regardless of what ancestors carry.
        {
            let raw = &git_repo.repo;
            let sig = git2::Signature::now("ddb", "ddb@local").unwrap();
            let parent = raw.head().unwrap().peel_to_commit().unwrap();
            let tree = parent.tree().unwrap();
            raw.commit(Some("HEAD"), &sig, &sig, "legacy commit, no trailer", &tree, &[&parent])
                .unwrap();
        }

        // Precondition: this HEAD carries no HLC trailer.
        {
            let head = git_repo.repo.head().unwrap().peel_to_commit().unwrap();
            assert!(
                crate::hlc::extract_hlc(head.message().unwrap()).is_none(),
                "precondition: this HEAD must have no HLC trailer"
            );
        }

        // Garble the machine-local state so `load` cannot seed from it either.
        let state = git_repo.repo.path().join("ddb-hlc");
        std::fs::write(&state, "not-a-valid-hlc-line\n").unwrap();

        // With no trailer to recover from, the seed is None: the first tick is
        // current wall-clock time, not a far-future constant.
        let clock = super::HlcClock::load(&git_repo.repo);
        let hlc = clock.tick();
        assert!(
            hlc.wall_ms > 1_600_000_000_000 && hlc.wall_ms < 10_000_000_000_000,
            "with no HEAD trailer, recovery must fall back to wall-clock time, got {}",
            hlc.wall_ms
        );
    }

    /// The ticked high-water mark is actually WRITTEN to `<git_dir>/ddb-hlc` on
    /// disk. An impl that keeps state only in memory (e.g. a process-global
    /// registry) passes every same-process test but loses all history on restart —
    /// fatal for short-lived CLI processes, each a fresh process. Reading the file
    /// back pins real cross-process persistence.
    #[test]
    fn tick_persists_high_water_mark_to_ddb_hlc_file() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let hi = super::HlcClock::load(&repo).tick();

        let raw = std::fs::read_to_string(repo.path().join("ddb-hlc"))
            .expect("tick must write ddb-hlc to disk");
        let persisted = Hlc::parse(raw.trim()).expect("ddb-hlc must hold a valid Hlc");
        assert!(
            persisted >= hi,
            "ddb-hlc must persist the high-water mark: {persisted} < {hi}"
        );
    }

    /// `load` reads a valid existing `ddb-hlc` back (simulating a fresh process
    /// inheriting the prior run's on-disk clock). On a HEADLESS repo the file is
    /// the ONLY possible seed source, so an impl that ignores the file and seeds
    /// only from HEAD/None ticks from bare wall-clock time and fails the exact
    /// derivation below.
    #[test]
    fn load_reads_back_a_valid_ddb_hlc_file() {
        let dir = TempDir::new().unwrap();
        // No HEAD → the file is the only possible seed source.
        let repo = git2::Repository::init(dir.path()).unwrap();
        let seeded = Hlc {
            wall_ms: u64::MAX / 2,
            counter: 9,
            node: "fileseed".into(),
        };
        std::fs::write(repo.path().join("ddb-hlc"), seeded.to_string()).unwrap();

        let clock = super::HlcClock::load(&repo);
        let next = clock.tick();
        assert_eq!(
            next.wall_ms, seeded.wall_ms,
            "load must seed from the valid ddb-hlc file, not ignore it: {} != {}",
            next.wall_ms, seeded.wall_ms
        );
        assert_eq!(
            next.counter,
            seeded.counter + 1,
            "tick must bump the file's counter, proving the file was read: {} != {}",
            next.counter,
            seeded.counter + 1
        );
    }

    /// When `<git_dir>/ddb-node` holds a uuid, a ticked Hlc's node is the first 8
    /// non-dash chars of that uuid (matching how `Hlc::now` truncates node ids).
    #[test]
    fn tick_node_reflects_ddb_node_uuid() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        std::fs::write(
            repo.path().join("ddb-node"),
            "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
        )
        .unwrap();

        let clock = super::HlcClock::load(&repo);
        let hlc = clock.tick();
        assert_eq!(
            hlc.node, "a1b2c3d4",
            "node must be the first 8 non-dash chars of the ddb-node uuid"
        );
    }

    /// With no `ddb-node` file, `load`/`tick` still succeed and produce a
    /// non-empty node (ephemeral mint). Two independent repos must mint DISTINCT
    /// ephemeral nodes: a hardcoded constant node id (which would satisfy a bare
    /// non-empty check) fails the distinctness assertion.
    #[test]
    fn tick_succeeds_with_nonempty_node_when_ddb_node_absent() {
        let dir1 = TempDir::new().unwrap();
        let repo1 = git2::Repository::init(dir1.path()).unwrap();
        assert!(!repo1.path().join("ddb-node").exists());

        let dir2 = TempDir::new().unwrap();
        let repo2 = git2::Repository::init(dir2.path()).unwrap();
        assert!(!repo2.path().join("ddb-node").exists());

        let hlc1 = super::HlcClock::load(&repo1).tick();
        let hlc2 = super::HlcClock::load(&repo2).tick();

        assert!(
            !hlc1.node.is_empty() && !hlc2.node.is_empty(),
            "tick must still produce a non-empty node when ddb-node is absent"
        );
        assert_ne!(
            hlc1.node, hlc2.node,
            "two repos with no ddb-node must mint distinct ephemeral nodes, not a shared constant: {} == {}",
            hlc1.node, hlc2.node
        );
    }

    /// `load` on a repo with no HEAD must not panic; with no HEAD and no
    /// `ddb-hlc`, the seed is None, so the first tick is bare current wall-clock
    /// time (a wide sane band rejects both a zero seed and a far-future seed).
    #[test]
    fn load_on_headless_repo_ticks_from_wall_clock_without_panic() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        assert!(
            repo.head().is_err(),
            "expected a headless repo for this case"
        );

        let clock = super::HlcClock::load(&repo);
        let hlc = clock.tick();

        assert!(
            hlc.wall_ms > 1_600_000_000_000 && hlc.wall_ms < 10_000_000_000_000,
            "empty-repo tick must be current wall-clock time (seed None), got {}",
            hlc.wall_ms
        );
    }
}
