use std::path::Path;

use git2::{IndexAddOption, Oid, Repository, Signature};

use crate::error::{DoogatError, Result};
use crate::types::{CommitHash, RepoConfig};

use super::{
    hlc_clock, validate_path, write_lock, DepthGuard, GitRepo, CONFIG_FILE,
    CURRENT_FORMAT_VERSION, VERSION_FILE, WRITE_LOCK_TIMEOUT,
};

impl GitRepo {
    /// Initialize a new ddb Git repository.
    pub fn init(path: &Path) -> Result<Self> {
        let repo = Repository::init(path)?;
        let hlc_clock = hlc_clock::HlcClock::load(&repo);
        let git_repo = Self {
            repo,
            path: path.to_path_buf(),
            skip_commit_graph: std::cell::Cell::new(false),
            session_commits: std::sync::atomic::AtomicU32::new(0),
            write_lock_depth: std::cell::Cell::new(0),
            uncounted_commits: std::cell::Cell::new(0),
            commit_graph_pending: std::cell::Cell::new(false),
            hlc_clock,
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
        let hlc_clock = hlc_clock::HlcClock::load(&repo);
        let git_repo = Self {
            repo,
            path: path.to_path_buf(),
            skip_commit_graph: std::cell::Cell::new(false),
            session_commits: std::sync::atomic::AtomicU32::new(0),
            write_lock_depth: std::cell::Cell::new(0),
            uncounted_commits: std::cell::Cell::new(0),
            commit_graph_pending: std::cell::Cell::new(false),
            hlc_clock,
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

        let parent_commit = self.head_commit();
        let parents: Vec<&git2::Commit> = match parent_commit {
            Some(ref c) => vec![c],
            None => vec![],
        };

        let oid = self.create_commit(message, &tree, &parents)?;
        Ok(oid)
    }

    fn signature(&self) -> Result<Signature<'_>> {
        self.repo
            .signature()
            .or_else(|_| Signature::now("ddb", "ddb@local").map_err(|e| e.into()))
    }

    /// Single chokepoint for non-merge write commits: tick the machine-local
    /// HLC clock, stamp its value as a trailer on `message`, and commit.
    pub(super) fn create_commit(
        &self,
        message: &str,
        tree: &git2::Tree,
        parents: &[&git2::Commit],
    ) -> Result<Oid> {
        let hlc = self.hlc_clock.tick();
        let msg = crate::hlc::append_hlc_trailer(message, &hlc);
        let sig = self.signature()?;
        Ok(self
            .repo
            .commit(Some("HEAD"), &sig, &sig, &msg, tree, parents)?)
    }

    pub(super) fn head_commit(&self) -> Option<git2::Commit<'_>> {
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
    ///
    /// Also exposed through [`crate::traits::GitBackend::with_write_lock`] so
    /// the service layer can run a single-row read → modify → commit inside
    /// one critical section (FT-5). The commit-graph rewrite and the
    /// write-threshold maintenance check that
    /// commits inside `f` (any nesting depth) request run only here, after
    /// the outermost lock is released, whether `f` succeeded or failed.
    pub(super) fn with_write_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.write_lock_depth.get() > 0 {
            // Already holding the process lock on this repo — re-entrant call.
            return f();
        }
        let result = {
            let _os_guard = write_lock::acquire(
                &self.path.join(".git"),
                "ddb-write.lock",
                WRITE_LOCK_TIMEOUT,
            )?;
            self.write_lock_depth.set(1);
            // Reset the depth on scope exit (incl. panic unwind) before the OS
            // guard drops, so the re-entrant flag never outlives the held lock.
            let _depth = DepthGuard(&self.write_lock_depth);
            f()
        };
        // Lock released. Graph rewrites coalesce into one; the maintenance
        // check runs once per commit so the session counter advances exactly
        // as it did when the check was inline.
        if self.commit_graph_pending.replace(false) {
            self.write_commit_graph();
        }
        for _ in 0..self.uncounted_commits.replace(0) {
            crate::maintenance::check_write_threshold(self);
        }
        result
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

            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self.create_commit(message, &tree, &[&parent])?;
            self.write_commit_graph();
            self.uncounted_commits.set(self.uncounted_commits.get() + 1);
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

            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self.create_commit(message, &tree, &[&parent])?;
            self.write_commit_graph();
            self.uncounted_commits.set(self.uncounted_commits.get() + 1);
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
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self.create_commit(message, &tree, &[&parent])?;
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
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self.create_commit(message, &tree, &[&parent])?;
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
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self.create_commit(message, &tree, &[&parent])?;
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
            let parent = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let oid = self.create_commit(message, &tree, &[&parent])?;
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
    /// Under the write lock (any nesting depth) the rewrite is recorded and
    /// run by the outermost `with_write_lock` after release, so the `git`
    /// subprocess never runs while the lock is held (FT-5).
    pub fn write_commit_graph(&self) {
        if self.skip_commit_graph.get() {
            return;
        }
        if self.write_lock_depth.get() > 0 {
            self.commit_graph_pending.set(true);
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
