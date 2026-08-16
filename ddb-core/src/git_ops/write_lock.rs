//! Repo-scoped, cross-process advisory write lock for the git commit paths.
//!
//! Every git write critical section (stage → `write_tree` → resolve parent →
//! `commit`) on one repo must hold this lock so that concurrent writers — a
//! downstream app running `ddb serve` while the user runs the CLI, a shell
//! firing two `ddb create`s, or several threads in one process — cannot lose
//! commits by resolving a stale parent and force-updating `HEAD`.
//!
//! The lock is an exclusive advisory lock on `<lock_dir>/<lock_name>`, taken via
//! [`fs2`], which speaks both Unix `flock` and Windows `LockFileEx`. (`rustix`'s
//! `flock` is Unix-only and would leave the Windows CI matrix unguarded.)
//!
//! The git write lock (`.git/ddb-write.lock`) and the index rebuild lock
//! (`.ddb/ddb-rebuild.lock`) are never held simultaneously by the same call
//! stack. If a future change needs both, fully release one before acquiring the
//! other — never acquire the second while the first is still held.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

use fs2::FileExt;

use crate::error::{DoogatError, Result};



/// Poll cadence while blocked on a contended lock. Uncontended acquires take
/// the fast path (first `try_lock_exclusive` succeeds, no sleep).
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Held exclusive advisory lock on a repo's write-lock file. The OS lock is
/// released when this guard is dropped — explicitly via [`FileExt::unlock`]
/// and, as a backstop, by closing the file descriptor when `file` drops.
#[derive(Debug)]
pub struct WriteLockGuard {
    file: File,
}

impl Drop for WriteLockGuard {
    fn drop(&mut self) {
        // Best-effort: releasing the file descriptor also releases the lock,
        // so a failure here cannot leak the lock and is not worth surfacing.
        let _ = FileExt::unlock(&self.file);
    }
}

/// Acquire an exclusive advisory lock on `<lock_dir>/<lock_name>`,
/// creating the lock file (and its parent directory) if absent.
///
/// Blocks up to `timeout`, polling with `try_lock_exclusive`, then fails loud
/// with a retryable [`DoogatError::Conflict`] rather than hanging forever.
pub fn acquire(lock_dir: &Path, lock_name: &str, timeout: Duration) -> Result<WriteLockGuard> {
    let lock_path = lock_dir.join(lock_name);
    // A real repo already has `.git/`; create it if missing so the primitive
    // is usable on a bare path (and so a fresh checkout never trips here).
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    let contended = fs2::lock_contended_error().kind();
    let start = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(WriteLockGuard { file }),
            Err(e) if e.kind() == contended => {
                if start.elapsed() >= timeout {
                    return Err(DoogatError::Conflict(format!(
                        "timed out after {}ms waiting for the repo write lock ({}); \
                         another ddb write is in progress",
                        timeout.as_millis(),
                        lock_path.display()
                    )));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(DoogatError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Name used by tests that only care about mutual exclusion, not identity.
    const TEST_LOCK_NAME: &str = "test.lock";

    /// The lock must exclude a second acquirer from the critical section until
    /// the first guard is released — the core mutual-exclusion guarantee.
    #[test]
    fn second_acquire_blocks_until_first_guard_released() {
        let dir = TempDir::new().unwrap();
        let lock_dir = dir.path().to_path_buf();

        // `in_section` is true exactly while thread 1 holds the lock. If the
        // lock is broken, thread 2 enters its section while this is still true.
        let in_section = Arc::new(AtomicBool::new(false));
        let violated = Arc::new(AtomicBool::new(false));

        let g1 = acquire(&lock_dir, TEST_LOCK_NAME, Duration::from_secs(5)).unwrap();
        in_section.store(true, SeqCst);

        let lock_dir2 = lock_dir.clone();
        let in_section2 = in_section.clone();
        let violated2 = violated.clone();
        let t2 = std::thread::spawn(move || {
            // Must block here until g1 is released below.
            let _g2 = acquire(&lock_dir2, TEST_LOCK_NAME, Duration::from_secs(5)).unwrap();
            if in_section2.load(SeqCst) {
                violated2.store(true, SeqCst);
            }
        });

        // Give thread 2 ample time to reach (and block on) `acquire`. If the
        // lock did not exclude it, it would enter while `in_section` is true.
        std::thread::sleep(Duration::from_millis(50));
        in_section.store(false, SeqCst);
        std::mem::drop(g1); // release → thread 2 may now proceed

        t2.join().unwrap();
        assert!(
            !violated.load(SeqCst),
            "second acquire entered the critical section while the first still held the lock"
        );
    }

    /// Contention past the timeout returns a `Conflict` promptly instead of
    /// hanging — the fail-loud guarantee.
    #[test]
    fn acquire_times_out_when_lock_held() {
        let dir = TempDir::new().unwrap();
        let lock_dir = dir.path().to_path_buf();

        let _g1 = acquire(&lock_dir, TEST_LOCK_NAME, Duration::from_secs(5)).unwrap();

        let start = Instant::now();
        let err = acquire(&lock_dir, TEST_LOCK_NAME, Duration::from_millis(100)).unwrap_err();
        let elapsed = start.elapsed();

        assert!(
            matches!(err, DoogatError::Conflict(_)),
            "expected Conflict on timeout, got {err:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(100),
            "returned before the timeout elapsed ({elapsed:?})"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "took far longer than the timeout, likely hung ({elapsed:?})"
        );
    }

    /// After the holder releases, a waiter acquires cleanly (lock is reusable).
    #[test]
    fn acquire_succeeds_after_release() {
        let dir = TempDir::new().unwrap();
        let lock_dir = dir.path().to_path_buf();

        let g1 = acquire(&lock_dir, TEST_LOCK_NAME, Duration::from_secs(5)).unwrap();
        std::mem::drop(g1);
        // Same-path re-acquire must succeed immediately now that it is free.
        let _g2 = acquire(&lock_dir, TEST_LOCK_NAME, Duration::from_millis(200)).unwrap();
    }

    /// Two different lock names inside the same directory are independent
    /// locks: holding one must not exclude an acquirer of the other. This is
    /// the whole point of generalizing `acquire` to take an explicit name.
    #[test]
    fn different_lock_names_in_same_directory_do_not_exclude() {
        let dir = TempDir::new().unwrap();
        let lock_dir = dir.path().to_path_buf();

        let _g1 = acquire(&lock_dir, "a.lock", Duration::from_secs(5)).unwrap();

        let start = Instant::now();
        // Must succeed promptly, not wait out the timeout: "b.lock" is a
        // distinct lock from "a.lock" in the same directory.
        let _g2 = acquire(&lock_dir, "b.lock", Duration::from_secs(2)).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "acquiring a different lock name blocked as if it shared the held lock ({elapsed:?})"
        );
    }

    /// The same lock name in two different directories does not exclude:
    /// the lock's identity is `<lock_dir>/<lock_name>`, not the name alone.
    #[test]
    fn same_lock_name_in_different_directories_does_not_exclude() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();

        let _g1 = acquire(dir_a.path(), TEST_LOCK_NAME, Duration::from_secs(5)).unwrap();

        let start = Instant::now();
        // Same name, different directory — must not contend with `_g1`.
        let _g2 = acquire(dir_b.path(), TEST_LOCK_NAME, Duration::from_secs(2)).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "acquiring the same lock name in a different directory blocked as if it shared \
             the held lock ({elapsed:?})"
        );
    }

    /// `acquire` creates `lock_dir` — including nested, not-yet-existing
    /// ancestors — rather than erroring when it is absent.
    #[test]
    fn acquire_creates_lock_dir_when_absent() {
        let dir = TempDir::new().unwrap();
        let lock_dir = dir.path().join("nested").join("does-not-exist-yet");
        assert!(!lock_dir.exists());

        let _g1 = acquire(&lock_dir, TEST_LOCK_NAME, Duration::from_secs(5)).unwrap();

        assert!(
            lock_dir.is_dir(),
            "acquire did not create the (nested) lock directory"
        );
        assert!(
            lock_dir.join(TEST_LOCK_NAME).is_file(),
            "acquire did not create the lock file inside the created directory"
        );
    }
}
