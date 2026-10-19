mod hlc_clock;
mod merge;
mod read;
mod remote;
mod rename;
mod repo_core;
mod trait_impls;
pub(crate) mod write_lock;

pub use remote::{set_libgit2_remote_timeouts, DEFAULT_REMOTE_TIMEOUT};
pub use rename::rename_doogat;

use std::path::{Path, PathBuf};
use std::time::Duration;

use git2::Repository;
// Test modules reach these through `use super::*`; production code moved to siblings.
#[cfg(test)]
use git2::{IndexAddOption, Oid, Signature};

use crate::error::{DoogatError, Result};
#[cfg(test)]
use crate::types::MergeResult;

/// Compute the Git-relative path for a doogat based on type and folder setting.
///
/// - `folder=true` + type → `ddb/{type}/{id}.md`
/// - `folder=false` or no type → `ddb/{id}.md`
pub fn doogat_path(id: &str, type_name: Option<&str>, folder: bool) -> String {
    if folder {
        if let Some(t) = type_name {
            return format!("ddb/{t}/{id}.md");
        }
    }
    format!("ddb/{id}.md")
}

/// Current repository format version. Incremented when on-disk layout changes.
pub const CURRENT_FORMAT_VERSION: u32 = 1;

const VERSION_FILE: &str = ".ddb-version";
const CONFIG_FILE: &str = ".ddb.toml";

/// Default bound on how long a git write waits for the repo write lock before
/// failing loud with a `Conflict`. Uncontended acquires are sub-millisecond;
/// this only bites under real cross-process write contention.
const WRITE_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

impl From<git2::Error> for DoogatError {
    fn from(e: git2::Error) -> Self {
        Self::Git(e.message().to_string())
    }
}

/// Reject symlinks, absolute paths, and paths that escape the repository root.
///
/// Works for both existing and not-yet-created paths:
/// 1. Rejects absolute paths (which would replace the base in `Path::join`).
/// 2. Component check catches `..` traversal regardless of file existence.
/// 3. For paths that exist on disk, also rejects symlinks and verifies
///    the canonical path stays within the repo root.
fn validate_path(repo_root: &Path, relative: &str) -> Result<()> {
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err(DoogatError::InvalidPath(format!(
            "absolute paths not allowed: {relative}"
        )));
    }
    for component in rel.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(DoogatError::InvalidPath(format!(
                "path escapes repository root: {relative}"
            )));
        }
    }

    let full = repo_root.join(relative);
    if let Ok(meta) = full.symlink_metadata() {
        if meta.file_type().is_symlink() {
            return Err(DoogatError::InvalidPath(format!(
                "symlinks not allowed: {relative}"
            )));
        }
        let canonical = full.canonicalize()?;
        let root_canonical = repo_root.canonicalize()?;
        if !canonical.starts_with(&root_canonical) {
            return Err(DoogatError::InvalidPath(format!(
                "path escapes repository root: {relative}"
            )));
        }
    }

    Ok(())
}

pub struct GitRepo {
    pub repo: Repository,
    pub path: PathBuf,
    skip_commit_graph: std::cell::Cell<bool>,
    session_commits: std::sync::atomic::AtomicU32,
    /// Re-entrancy depth for [`GitRepo::with_write_lock`]. `GitRepo` is `!Sync`
    /// (it holds `Cell`s), so only the owning thread ever touches this. It lets
    /// one locked write path call another (`commit_file` → `commit_files`,
    /// `rename_file` → `commit_batch`) without the process deadlocking on its
    /// own advisory lock.
    write_lock_depth: std::cell::Cell<u32>,
    /// Commits made under the lock that `maintenance::check_write_threshold`
    /// has not counted yet. The counting paths bump this instead of calling
    /// it inline; the outermost [`GitRepo::with_write_lock`] drains it AFTER
    /// releasing the lock, one check per commit, so the `git maintenance`
    /// subprocess never runs under the write lock and the session counter
    /// still advances once per commit (FT-5).
    uncounted_commits: std::cell::Cell<u32>,
    /// A `write_commit_graph` request made while the lock was held. Drained
    /// by the outermost `with_write_lock` after release into one rewrite,
    /// so the `git commit-graph write` subprocess never runs under the lock.
    commit_graph_pending: std::cell::Cell<bool>,
    /// Machine-local, monotonic HLC clock persisted to `<git_dir>/ddb-hlc`.
    /// Every non-merge write commit ticks it and stamps the result as a trailer.
    hlc_clock: hlc_clock::HlcClock,
}

/// Resets a `GitRepo`'s write-lock re-entrancy depth to zero on scope exit,
/// so the flag is cleared even if the wrapped closure panics.
struct DepthGuard<'a>(&'a std::cell::Cell<u32>);

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.0.set(0);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod remote_timeout_tests;

/// FT-5: the `GitBackend::with_write_lock` hook and the subprocess deferral
/// (commit-graph rewrite, write-threshold maintenance) it relies on. Kept
/// next to the lock so a change to either is reviewed against both.
#[cfg(test)]
mod lock_hook_tests;
