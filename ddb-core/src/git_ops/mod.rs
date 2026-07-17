mod merge;
mod read;
mod remote;
mod rename;
mod write_lock;

pub use rename::rename_doogat;

use std::path::{Path, PathBuf};
use std::time::Duration;

use git2::{IndexAddOption, Oid, Repository, Signature};

use crate::error::{DoogatError, Result};
use crate::types::{CommitHash, MergeResult, RepoConfig};

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
}

/// Resets a `GitRepo`'s write-lock re-entrancy depth to zero on scope exit,
/// so the flag is cleared even if the wrapped closure panics.
struct DepthGuard<'a>(&'a std::cell::Cell<u32>);

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.0.set(0);
    }
}

impl GitRepo {
    /// Initialize a new ddb Git repository.
    pub fn init(path: &Path) -> Result<Self> {
        let repo = Repository::init(path)?;
        let git_repo = Self {
            repo,
            path: path.to_path_buf(),
            skip_commit_graph: std::cell::Cell::new(false),
            session_commits: std::sync::atomic::AtomicU32::new(0),
            write_lock_depth: std::cell::Cell::new(0),
        };

        // Create standard directories with .gitkeep
        for dir in &["ddb", "reference", ".nodes", ".crdt/temp"] {
            let dir_path = path.join(dir);
            std::fs::create_dir_all(&dir_path)?;
            std::fs::write(dir_path.join(".gitkeep"), "")?;
        }

        // Add .ddb/ to .gitignore
        let gitignore_path = path.join(".gitignore");
        let existing = if gitignore_path.exists() {
            std::fs::read_to_string(&gitignore_path)?
        } else {
            String::new()
        };
        if !existing.contains(".ddb/") {
            let content = if existing.is_empty() {
                ".ddb/\n".to_string()
            } else {
                format!("{existing}\n.ddb/\n")
            };
            std::fs::write(&gitignore_path, content)?;
        }

        // Write format version file
        std::fs::write(path.join(VERSION_FILE), CURRENT_FORMAT_VERSION.to_string())?;

        // Write default repo config
        let default_config = RepoConfig::default();
        let config_toml = toml::to_string_pretty(&default_config)
            .map_err(|e| DoogatError::Toml(e.to_string()))?;
        std::fs::write(path.join(CONFIG_FILE), &config_toml)?;

        // Stage everything and create initial commit
        git_repo.commit_all("init: ddb repository")?;

        Ok(git_repo)
    }

    /// Open an existing ddb Git repository.
    /// Checks format version: rejects repos newer than driver, auto-upgrades v0→v1.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::open(path)?;
        let git_repo = Self {
            repo,
            path: path.to_path_buf(),
            skip_commit_graph: std::cell::Cell::new(false),
            session_commits: std::sync::atomic::AtomicU32::new(0),
            write_lock_depth: std::cell::Cell::new(0),
        };

        git_repo.check_format_version()?;
        git_repo.cleanup_orphaned_crdt_temp();
        tracing::debug!(path = %path.display(), "repo_opened");

        Ok(git_repo)
    }

    /// Read repo format version, migrate if needed, reject if too new.
    fn check_format_version(&self) -> Result<()> {
        let version = match self.read_file(VERSION_FILE) {
            Ok(content) => content
                .trim()
                .parse::<u32>()
                .map_err(|e| DoogatError::Parse(format!("bad version file: {e}")))?,
            Err(_) => 0, // missing file = pre-version repo
        };

        if version > CURRENT_FORMAT_VERSION {
            return Err(DoogatError::VersionMismatch {
                repo: version,
                driver: CURRENT_FORMAT_VERSION,
            });
        }

        if version < CURRENT_FORMAT_VERSION {
            self.migrate_format(version)?;
        }

        Ok(())
    }

    /// Run format migrations from `from_version` up to CURRENT_FORMAT_VERSION.
    fn migrate_format(&self, from_version: u32) -> Result<()> {
        let mut v = from_version;
        while v < CURRENT_FORMAT_VERSION {
            match v {
                0 => {
                    // v0 → v1: write version file
                    self.commit_file(
                        VERSION_FILE,
                        &CURRENT_FORMAT_VERSION.to_string(),
                        "migrate: add .ddb-version (v0 → v1)",
                    )?;
                }
                _ => {
                    return Err(DoogatError::Parse(format!(
                        "unknown format version {v}, cannot migrate"
                    )));
                }
            }
            v += 1;
        }
        Ok(())
    }

    /// Stage all files and create a commit.
    fn commit_all(&self, message: &str) -> Result<Oid> {
        let mut index = self.repo.index()?;
        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;

        let parent_commit = self.head_commit();
        let parents: Vec<&git2::Commit> = match parent_commit {
            Some(ref c) => vec![c],
            None => vec![],
        };

        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
        Ok(oid)
    }

    fn signature(&self) -> Result<Signature<'_>> {
        self.repo
            .signature()
            .or_else(|_| Signature::now("ddb", "ddb@local").map_err(|e| e.into()))
    }

    fn head_commit(&self) -> Option<git2::Commit<'_>> {
        self.repo.head().ok().and_then(|h| h.peel_to_commit().ok())
    }

    /// Load the repo index, rebasing it on the HEAD tree so that entries
    /// modified by external git operations (CLI, concurrent tools) are visible.
    /// Without this, the in-memory index cache can silently revert those changes
    /// on the next commit.
    fn fresh_index(&self) -> Result<git2::Index> {
        let head_tree = self.head_commit().and_then(|c| c.tree().ok());
        let mut index = self.repo.index()?;
        if let Some(tree) = head_tree {
            index.read_tree(&tree)?;
        }
        Ok(index)
    }

    /// Get current HEAD as a domain-level CommitHash.
    pub fn head_oid(&self) -> Result<CommitHash> {
        let head = self.repo.head()?;
        Ok(CommitHash(head.peel_to_commit()?.id().to_string()))
    }

    /// Run `f` while holding the repo-scoped, cross-process write lock.
    ///
    /// Serializes the git write critical section (stage → `write_tree` →
    /// resolve parent → `commit`) against other processes and threads so a
    /// stale-parent commit can't force-update `HEAD` and silently drop a peer's
    /// write. See [`write_lock`].
    ///
    /// Re-entrant on the owning thread: a write path that calls another
    /// (`commit_file` → `commit_files`, `rename_file` → `commit_batch`) runs
    /// the inner body directly instead of re-acquiring, because a second
    /// exclusive advisory lock from the same process would self-deadlock.
    ///
    /// Ordering invariant: this lock is always the innermost resource. The
    /// SINGLETON create/upsert paths take a SQLite `BEGIN IMMEDIATE` first and
    /// then commit (SQLite-outer, git-inner); no path takes this lock and then
    /// opens a SQLite immediate transaction, so the two cannot deadlock.
    fn with_write_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.write_lock_depth.get() > 0 {
            // Already holding the process lock on this repo — re-entrant call.
            return f();
        }
        let _os_guard = write_lock::acquire(&self.path, WRITE_LOCK_TIMEOUT)?;
        self.write_lock_depth.set(1);
        // Reset the depth on scope exit (incl. panic unwind) before the OS
        // guard drops, so the re-entrant flag never outlives the held lock.
        let _depth = DepthGuard(&self.write_lock_depth);
        f()
    }

    /// Write a file, stage it, and commit.
    pub fn commit_file(&self, rel_path: &str, content: &str, message: &str) -> Result<CommitHash> {
        self.with_write_lock(|| self.commit_files(&[(rel_path, content)], message))
    }

    /// Write binary content to a file, stage it, and commit.
    pub fn commit_binary_file(
        &self,
        rel_path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<CommitHash> {
        self.with_write_lock(|| {
            validate_path(&self.path, rel_path)?;
            let full_path = self.path.join(rel_path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, bytes)?;

            let mut index = self.fresh_index()?;
            index.add_path(Path::new(rel_path))?;
            index.write()?;
            let tree_oid = index.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let sig = self.signature()?;

            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self
                .repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
            self.write_commit_graph();
            crate::maintenance::check_write_threshold(self);
            Ok(CommitHash(oid.to_string()))
        })
    }

    /// Write multiple files, stage them, and commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_files(&self, files: &[(&str, &str)], message: &str) -> Result<CommitHash> {
        self.with_write_lock(|| {
            for (rel_path, _) in files {
                validate_path(&self.path, rel_path)?;
            }
            for (rel_path, content) in files {
                let full_path = self.path.join(rel_path);
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&full_path, content)?;
            }

            let mut index = self.fresh_index()?;
            for (rel_path, _) in files {
                index.add_path(Path::new(rel_path))?;
            }
            index.write()?;
            let tree_oid = index.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let sig = self.signature()?;

            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self
                .repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
            self.write_commit_graph();
            crate::maintenance::check_write_threshold(self);
            Ok(CommitHash(oid.to_string()))
        })
    }

    /// List all .md files under ddb/ in the HEAD tree.
    pub fn list_doogats(&self) -> Result<Vec<String>> {
        let head = self.repo.head()?.peel_to_commit()?;
        let tree = head.tree()?;
        let mut paths = Vec::new();

        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            let full_path = format!("{}{}", dir, entry.name().unwrap_or(""));
            if full_path.starts_with("ddb/") && full_path.ends_with(".md") {
                paths.push(full_path);
            }
            git2::TreeWalkResult::Ok
        })?;

        Ok(paths)
    }

    /// Delete a file, stage the removal, and commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn delete_file(&self, rel_path: &str, message: &str) -> Result<CommitHash> {
        self.with_write_lock(|| {
            validate_path(&self.path, rel_path)?;
            let full_path = self.path.join(rel_path);
            if full_path.exists() {
                std::fs::remove_file(&full_path)?;
            }
            let mut index = self.fresh_index()?;
            index.remove_path(Path::new(rel_path))?;
            index.write()?;
            let tree_oid = index.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let sig = self.signature()?;
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self
                .repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
            self.write_commit_graph();
            Ok(CommitHash(oid.to_string()))
        })
    }

    /// Delete multiple files, stage the removals, and commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn delete_files(&self, paths: &[&str], message: &str) -> Result<CommitHash> {
        self.with_write_lock(|| {
            for rel_path in paths {
                validate_path(&self.path, rel_path)?;
            }
            for rel_path in paths {
                let full_path = self.path.join(rel_path);
                if full_path.exists() {
                    std::fs::remove_file(&full_path)?;
                }
            }
            let mut index = self.fresh_index()?;
            for rel_path in paths {
                index.remove_path(Path::new(rel_path))?;
            }
            index.write()?;
            let tree_oid = index.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let sig = self.signature()?;
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self
                .repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
            self.write_commit_graph();
            Ok(CommitHash(oid.to_string()))
        })
    }

    /// Rename (move) a file in git: read old content, write to new path, delete old.
    pub fn rename_file(&self, old_path: &str, new_path: &str, message: &str) -> Result<CommitHash> {
        self.with_write_lock(|| {
            validate_path(&self.path, old_path)?;
            validate_path(&self.path, new_path)?;
            let full_old = self.path.join(old_path);
            let full_new = self.path.join(new_path);
            if !full_old.exists() {
                return Err(DoogatError::NotFound(old_path.to_string()));
            }
            if full_new.exists() {
                return Err(DoogatError::InvalidPath(format!(
                    "target path already exists: {new_path}"
                )));
            }
            let content = std::fs::read_to_string(&full_old)?;
            self.commit_batch(&[(new_path, &content)], &[old_path], message)
        })
    }

    /// Write and/or delete multiple files in a single commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_batch(
        &self,
        writes: &[(&str, &str)],
        deletes: &[&str],
        message: &str,
    ) -> Result<CommitHash> {
        self.with_write_lock(|| {
            for (rel_path, _) in writes {
                validate_path(&self.path, rel_path)?;
            }
            for rel_path in deletes {
                validate_path(&self.path, rel_path)?;
            }
            for (rel_path, content) in writes {
                let full_path = self.path.join(rel_path);
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&full_path, content)?;
            }
            for rel_path in deletes {
                let full_path = self.path.join(rel_path);
                if full_path.exists() {
                    std::fs::remove_file(&full_path)?;
                }
            }
            let mut index = self.fresh_index()?;
            for (rel_path, _) in writes {
                index.add_path(Path::new(rel_path))?;
            }
            for rel_path in deletes {
                index.remove_path(Path::new(rel_path))?;
            }
            index.write()?;
            let tree_oid = index.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let sig = self.signature()?;
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self
                .repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
            self.write_commit_graph();
            Ok(CommitHash(oid.to_string()))
        })
    }

    /// Write a binary file and one or more text files in a single atomic commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_binary_and_text(
        &self,
        binary_path: &str,
        bytes: &[u8],
        text_files: &[(&str, &str)],
        message: &str,
    ) -> Result<CommitHash> {
        self.with_write_lock(|| {
            validate_path(&self.path, binary_path)?;
            for (rel_path, _) in text_files {
                validate_path(&self.path, rel_path)?;
            }
            // Write binary
            let full = self.path.join(binary_path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, bytes)?;

            // Write text files
            for (rel_path, content) in text_files {
                let full_path = self.path.join(rel_path);
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&full_path, content)?;
            }

            let mut index = self.fresh_index()?;
            index.add_path(Path::new(binary_path))?;
            for (rel_path, _) in text_files {
                index.add_path(Path::new(rel_path))?;
            }
            index.write()?;
            let tree_oid = index.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let sig = self.signature()?;
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self
                .repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
            self.write_commit_graph();
            Ok(CommitHash(oid.to_string()))
        })
    }

    /// Remove any orphaned files in `.crdt/temp/` (best-effort, logs warnings).
    fn cleanup_orphaned_crdt_temp(&self) {
        let temp_dir = self.path.join(".crdt/temp");
        if !temp_dir.exists() {
            return;
        }
        let entries = match std::fs::read_dir(&temp_dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".gitkeep" {
                continue;
            }
            tracing::warn!("removing orphaned CRDT temp file: {name}");
            let _ = std::fs::remove_file(entry.path());
        }
    }

    /// Suppress per-commit `write_commit_graph` calls (for batch operations like sync).
    /// Call `write_commit_graph` explicitly once when done.
    pub fn set_skip_commit_graph(&self, skip: bool) {
        self.skip_commit_graph.set(skip);
    }

    /// Increment the session commit counter and return the new value.
    pub fn increment_session_commits(&self) -> u32 {
        self.session_commits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// Reset the session commit counter to zero.
    pub fn reset_session_commits(&self) {
        self.session_commits
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Write the commit-graph file for faster traversal (merge-base, log).
    /// Best-effort: silently ignored if `git` CLI unavailable.
    /// Skipped when `skip_commit_graph` flag is set (for batch operations).
    pub fn write_commit_graph(&self) {
        if self.skip_commit_graph.get() {
            return;
        }
        self.write_commit_graph_unconditional();
    }

    fn write_commit_graph_unconditional(&self) {
        let _ = std::process::Command::new("git")
            .args(["commit-graph", "write", "--reachable"])
            .current_dir(&self.path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

impl crate::traits::DoogatSource for GitRepo {
    fn list_doogats(&self) -> Result<Vec<String>> {
        self.list_doogats()
    }

    fn read_file(&self, path: &str) -> Result<String> {
        self.read_file(path)
    }

    fn head_oid(&self) -> Result<CommitHash> {
        self.head_oid()
    }

    fn diff_paths(
        &self,
        old_oid: &str,
        new_oid: &str,
    ) -> Result<Vec<(crate::types::DiffKind, String)>> {
        self.diff_paths(old_oid, new_oid)
    }

    fn read_files_batch(&self, paths: &[String]) -> Result<Vec<(String, Result<String>)>> {
        self.read_files_batch(paths)
    }
}

impl crate::traits::DoogatStore for GitRepo {
    fn commit_file(&self, path: &str, content: &str, msg: &str) -> Result<CommitHash> {
        self.commit_file(path, content, msg)
    }

    fn commit_files(&self, files: &[(&str, &str)], msg: &str) -> Result<CommitHash> {
        self.commit_files(files, msg)
    }

    fn delete_file(&self, path: &str, msg: &str) -> Result<CommitHash> {
        self.delete_file(path, msg)
    }

    fn delete_files(&self, paths: &[&str], msg: &str) -> Result<CommitHash> {
        self.delete_files(paths, msg)
    }

    fn commit_batch(
        &self,
        writes: &[(&str, &str)],
        deletes: &[&str],
        msg: &str,
    ) -> Result<CommitHash> {
        self.commit_batch(writes, deletes, msg)
    }
}

impl crate::traits::GitRemote for GitRepo {
    fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.add_remote(name, url)
    }

    fn fetch(&self, remote: &str, branch: &str) -> Result<()> {
        self.fetch(remote, branch)
    }

    fn push(&self, remote: &str, branch: &str) -> Result<()> {
        self.push(remote, branch)
    }
}

impl crate::traits::GitMerge for GitRepo {
    fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        self.merge_remote(remote, branch)
    }

    fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary: &[(&str, &str)],
        message: &str,
        theirs: &CommitHash,
    ) -> Result<CommitHash> {
        self.commit_merge(files, binary, message, theirs)
    }
}

impl crate::traits::GitHistory for GitRepo {
    fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        let oid_a = Oid::from_str(a)?;
        let oid_b = Oid::from_str(b)?;
        let base = self.repo.merge_base(oid_a, oid_b)?;
        Ok(base.to_string())
    }

    fn commit_parent_count(&self, commit_oid: &str) -> Result<usize> {
        let oid = Oid::from_str(commit_oid)?;
        let commit = self.repo.find_commit(oid)?;
        Ok(commit.parent_count())
    }

    fn commit_parent_oid(&self, commit_oid: &str, n: usize) -> Result<String> {
        let oid = Oid::from_str(commit_oid)?;
        let commit = self.repo.find_commit(oid)?;
        let parent = commit.parent(n)?;
        Ok(parent.id().to_string())
    }

    fn read_file_at(&self, commit_oid: &str, rel_path: &str) -> Result<String> {
        let oid = Oid::from_str(commit_oid)?;
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;
        let entry = tree
            .get_path(Path::new(rel_path))
            .map_err(|_| DoogatError::NotFound(rel_path.to_string()))?;
        let blob = self
            .repo
            .find_blob(entry.id())
            .map_err(|_| DoogatError::NotFound(rel_path.to_string()))?;
        let content =
            std::str::from_utf8(blob.content()).map_err(|e| DoogatError::Parse(e.to_string()))?;
        Ok(content.to_string())
    }

    fn walk_tree_files(&self, commit_oid: &str, prefix: &str) -> Result<Vec<(String, String)>> {
        let oid = Oid::from_str(commit_oid)?;
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;
        let mut files = Vec::new();
        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            let full_path = format!("{}{}", dir, entry.name().unwrap_or(""));
            if full_path.starts_with(prefix) {
                if let Ok(blob) = self.repo.find_blob(entry.id()) {
                    if let Ok(content) = std::str::from_utf8(blob.content()) {
                        files.push((full_path, content.to_string()));
                    }
                }
            }
            git2::TreeWalkResult::Ok
        })?;
        Ok(files)
    }

    fn find_hlc_for_path(&self, commit_oid: &str, path: &str) -> Option<crate::hlc::Hlc> {
        let oid = Oid::from_str(commit_oid).ok()?;
        let commit = self.repo.find_commit(oid).ok()?;
        self.find_hlc_for_path(&commit, path)
    }

    fn revision_date(&self, rel_path: &str) -> Result<Option<String>> {
        self.revision_date(rel_path)
    }
}

impl crate::traits::GitBinary for GitRepo {
    fn commit_binary_file(
        &self,
        rel_path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<CommitHash> {
        self.commit_binary_file(rel_path, bytes, message)
    }

    fn commit_binary_and_text(
        &self,
        binary_path: &str,
        bytes: &[u8],
        text_files: &[(&str, &str)],
        message: &str,
    ) -> Result<CommitHash> {
        self.commit_binary_and_text(binary_path, bytes, text_files, message)
    }

    fn read_blob(&self, oid_str: &str) -> Result<Vec<u8>> {
        self.read_blob(oid_str)
    }
}

impl crate::traits::GitRename for GitRepo {
    fn rename_file(&self, old_path: &str, new_path: &str, message: &str) -> Result<CommitHash> {
        self.rename_file(old_path, new_path, message)
    }
}

impl crate::traits::GitDesktopHooks for GitRepo {
    fn set_skip_commit_graph(&self, skip: bool) {
        self.set_skip_commit_graph(skip);
    }

    fn write_commit_graph(&self) {
        self.write_commit_graph();
    }

    fn increment_session_commits(&self) -> u32 {
        self.increment_session_commits()
    }

    fn reset_session_commits(&self) {
        self.reset_session_commits();
    }
}

impl crate::traits::GitBackend for GitRepo {
    fn repo_path(&self) -> &Path {
        &self.path
    }

    fn load_config(&self) -> Result<RepoConfig> {
        self.load_config()
    }
}

#[cfg(test)]
mod tests;
