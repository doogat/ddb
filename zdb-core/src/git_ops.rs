use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Oid, Repository, Signature};

use crate::error::{Result, ZettelError};
use crate::types::{CommitHash, ConflictFile, MergeResult, RenameReport, RepoConfig};

/// Compute the Git-relative path for a zettel based on type and folder setting.
///
/// - `folder=true` + type → `zettelkasten/{type}/{id}.md`
/// - `folder=false` or no type → `zettelkasten/{id}.md`
pub fn zettel_path(id: &str, type_name: Option<&str>, folder: bool) -> String {
    if folder {
        if let Some(t) = type_name {
            return format!("zettelkasten/{t}/{id}.md");
        }
    }
    format!("zettelkasten/{id}.md")
}

/// Current repository format version. Incremented when on-disk layout changes.
pub const CURRENT_FORMAT_VERSION: u32 = 1;

const VERSION_FILE: &str = ".zetteldb-version";
const CONFIG_FILE: &str = ".zetteldb.toml";

impl From<git2::Error> for ZettelError {
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
        return Err(ZettelError::InvalidPath(format!(
            "absolute paths not allowed: {relative}"
        )));
    }
    for component in rel.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(ZettelError::InvalidPath(format!(
                "path escapes repository root: {relative}"
            )));
        }
    }

    let full = repo_root.join(relative);
    if let Ok(meta) = full.symlink_metadata() {
        if meta.file_type().is_symlink() {
            return Err(ZettelError::InvalidPath(format!(
                "symlinks not allowed: {relative}"
            )));
        }
        let canonical = full.canonicalize()?;
        let root_canonical = repo_root.canonicalize()?;
        if !canonical.starts_with(&root_canonical) {
            return Err(ZettelError::InvalidPath(format!(
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
}

impl GitRepo {
    /// Initialize a new zettelkasten Git repository.
    pub fn init(path: &Path) -> Result<Self> {
        let repo = Repository::init(path)?;
        let git_repo = Self {
            repo,
            path: path.to_path_buf(),
            skip_commit_graph: std::cell::Cell::new(false),
            session_commits: std::sync::atomic::AtomicU32::new(0),
        };

        // Create standard directories with .gitkeep
        for dir in &["zettelkasten", "reference", ".nodes", ".crdt/temp"] {
            let dir_path = path.join(dir);
            std::fs::create_dir_all(&dir_path)?;
            std::fs::write(dir_path.join(".gitkeep"), "")?;
        }

        // Add .zdb/ to .gitignore
        let gitignore_path = path.join(".gitignore");
        let existing = if gitignore_path.exists() {
            std::fs::read_to_string(&gitignore_path)?
        } else {
            String::new()
        };
        if !existing.contains(".zdb/") {
            let content = if existing.is_empty() {
                ".zdb/\n".to_string()
            } else {
                format!("{existing}\n.zdb/\n")
            };
            std::fs::write(&gitignore_path, content)?;
        }

        // Write format version file
        std::fs::write(path.join(VERSION_FILE), CURRENT_FORMAT_VERSION.to_string())?;

        // Write default repo config
        let default_config = RepoConfig::default();
        let config_toml = toml::to_string_pretty(&default_config)
            .map_err(|e| ZettelError::Toml(e.to_string()))?;
        std::fs::write(path.join(CONFIG_FILE), &config_toml)?;

        // Stage everything and create initial commit
        git_repo.commit_all("init: zettelkasten repository")?;

        Ok(git_repo)
    }

    /// Open an existing zettelkasten Git repository.
    /// Checks format version: rejects repos newer than driver, auto-upgrades v0→v1.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::open(path)?;
        let git_repo = Self {
            repo,
            path: path.to_path_buf(),
            skip_commit_graph: std::cell::Cell::new(false),
            session_commits: std::sync::atomic::AtomicU32::new(0),
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
                .map_err(|e| ZettelError::Parse(format!("bad version file: {e}")))?,
            Err(_) => 0, // missing file = pre-version repo
        };

        if version > CURRENT_FORMAT_VERSION {
            return Err(ZettelError::VersionMismatch {
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
                        "migrate: add .zetteldb-version (v0 → v1)",
                    )?;
                }
                _ => {
                    return Err(ZettelError::Parse(format!(
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
            .or_else(|_| Signature::now("zdb", "zdb@local").map_err(|e| e.into()))
    }

    fn head_commit(&self) -> Option<git2::Commit<'_>> {
        self.repo.head().ok().and_then(|h| h.peel_to_commit().ok())
    }

    /// Get current HEAD as a domain-level CommitHash.
    pub fn head_oid(&self) -> Result<CommitHash> {
        let head = self.repo.head()?;
        Ok(CommitHash(head.peel_to_commit()?.id().to_string()))
    }

    /// Write a file, stage it, and commit.
    pub fn commit_file(&self, rel_path: &str, content: &str, message: &str) -> Result<CommitHash> {
        self.commit_files(&[(rel_path, content)], message)
    }

    /// Write binary content to a file, stage it, and commit.
    pub fn commit_binary_file(
        &self,
        rel_path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<CommitHash> {
        validate_path(&self.path, rel_path)?;
        let full_path = self.path.join(rel_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, bytes)?;

        let mut index = self.repo.index()?;
        index.add_path(Path::new(rel_path))?;
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;

        let parent = self
            .head_commit()
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        self.write_commit_graph();
        crate::maintenance::check_write_threshold(self);
        Ok(CommitHash(oid.to_string()))
    }

    /// Write multiple files, stage them, and commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_files(&self, files: &[(&str, &str)], message: &str) -> Result<CommitHash> {
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

        let mut index = self.repo.index()?;
        for (rel_path, _) in files {
            index.add_path(Path::new(rel_path))?;
        }
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;

        let parent = self
            .head_commit()
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        self.write_commit_graph();
        crate::maintenance::check_write_threshold(self);
        Ok(CommitHash(oid.to_string()))
    }

    /// Write resolved files and create a merge commit with two parents.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_merge(
        &self,
        files: &[(&str, &str)],
        message: &str,
        theirs: &CommitHash,
    ) -> Result<CommitHash> {
        for (rel_path, _) in files {
            validate_path(&self.path, rel_path)?;
        }

        let our_commit = self
            .head_commit()
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let theirs_oid = Oid::from_str(&theirs.0)?;
        let their_commit = self.repo.find_commit(theirs_oid)?;

        // Write resolved files to disk
        for (rel_path, content) in files {
            let full_path = self.path.join(rel_path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, content)?;
        }

        // Write non-conflicting files from theirs (Added or Modified on
        // theirs while unchanged on ours). Without this, remote-only
        // changes are missing from the merge commit tree.
        let ours_tree = our_commit.tree()?;
        let theirs_tree = their_commit.tree()?;
        let diff =
            self.repo
                .diff_tree_to_tree(Some(&ours_tree), Some(&theirs_tree), None)?;
        let resolved_set: std::collections::HashSet<&str> =
            files.iter().map(|(p, _)| *p).collect();
        let mut theirs_only = Vec::new();
        for delta in diff.deltas() {
            let dominated = matches!(
                delta.status(),
                git2::Delta::Added | git2::Delta::Modified
            );
            if dominated {
                if let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) {
                    if !resolved_set.contains(path) {
                        if let Ok(blob) = self.repo.find_blob(delta.new_file().id()) {
                            let full = self.path.join(path);
                            if let Some(parent) = full.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            std::fs::write(&full, blob.content())?;
                            theirs_only.push(path.to_string());
                        }
                    }
                }
            }
        }

        let mut index = self.repo.index()?;
        for (rel_path, _) in files {
            index.add_path(Path::new(rel_path))?;
        }
        for path in &theirs_only {
            index.add_path(Path::new(path))?;
        }
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;

        let oid = self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            message,
            &tree,
            &[&our_commit, &their_commit],
        )?;
        self.write_commit_graph();
        Ok(CommitHash(oid.to_string()))
    }

    /// List all .md files under zettelkasten/ in the HEAD tree.
    pub fn list_zettels(&self) -> Result<Vec<String>> {
        let head = self.repo.head()?.peel_to_commit()?;
        let tree = head.tree()?;
        let mut paths = Vec::new();

        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            let full_path = format!("{}{}", dir, entry.name().unwrap_or(""));
            if full_path.starts_with("zettelkasten/") && full_path.ends_with(".md") {
                paths.push(full_path);
            }
            git2::TreeWalkResult::Ok
        })?;

        Ok(paths)
    }

    /// Add a named remote.
    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.repo.remote(name, url)?;
        Ok(())
    }

    /// Fetch from a remote.
    pub fn fetch(&self, remote: &str, branch: &str) -> Result<()> {
        let mut remote = self.repo.find_remote(remote)?;
        remote.fetch(&[branch], None, None)?;
        Ok(())
    }

    /// Push to a remote.
    pub fn push(&self, remote: &str, branch: &str) -> Result<()> {
        let mut remote = self.repo.find_remote(remote)?;
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        remote.push(&[&refspec], None)?;
        Ok(())
    }

    /// Merge a fetched remote branch, returning the merge result.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        let fetch_head_ref = format!("refs/remotes/{remote}/{branch}");
        let reference = self
            .repo
            .find_reference(&fetch_head_ref)
            .map_err(|_| ZettelError::NotFound(fetch_head_ref.clone()))?;
        let annotated = self.repo.reference_to_annotated_commit(&reference)?;

        let (analysis, _pref) = self.repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(MergeResult::AlreadyUpToDate);
        }

        if analysis.is_fast_forward() {
            let target_oid = annotated.id();
            let mut reference = self
                .repo
                .find_reference("refs/heads/master")
                .or_else(|_| self.repo.find_reference("HEAD"))?;
            reference.set_target(target_oid, "fast-forward")?;
            self.repo.set_head("refs/heads/master")?;
            self.repo
                .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            self.write_commit_graph();
            return Ok(MergeResult::FastForward(CommitHash(target_oid.to_string())));
        }

        // Normal merge
        let their_commit = self.repo.find_commit(annotated.id())?;
        let our_commit = self
            .head_commit()
            .ok_or_else(|| ZettelError::Parse("no HEAD".into()))?;
        let _ancestor = self.repo.merge_base(our_commit.id(), their_commit.id())?;

        let mut merge_index = self.repo.merge_commits(&our_commit, &their_commit, None)?;

        if merge_index.has_conflicts() {
            let conflicts = self.extract_conflicts(&merge_index, &our_commit, &their_commit)?;
            // Clean up merge state
            self.repo.cleanup_state()?;
            return Ok(MergeResult::Conflicts(
                conflicts,
                CommitHash(their_commit.id().to_string()),
            ));
        }

        // Clean merge — write tree and commit
        let tree_oid = merge_index.write_tree_to(&self.repo)?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;
        let oid = self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("merge {remote}/{branch}"),
            &tree,
            &[&our_commit, &their_commit],
        )?;
        self.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
        self.write_commit_graph();
        Ok(MergeResult::Clean(CommitHash(oid.to_string())))
    }

    fn extract_conflicts(
        &self,
        index: &git2::Index,
        ours_commit: &git2::Commit,
        theirs_commit: &git2::Commit,
    ) -> Result<Vec<ConflictFile>> {
        let mut conflicts = Vec::new();
        for conflict in index.conflicts()? {
            let conflict = conflict?;
            let path = conflict
                .our
                .as_ref()
                .or(conflict.their.as_ref())
                .and_then(|e| String::from_utf8(e.path.clone()).ok())
                .unwrap_or_default();

            let ancestor = match conflict.ancestor {
                Some(ref entry) => {
                    let blob = self.repo.find_blob(entry.id)?;
                    Some(String::from_utf8_lossy(blob.content()).to_string())
                }
                None => None,
            };
            let ours = match conflict.our {
                Some(ref entry) => {
                    let blob = self.repo.find_blob(entry.id)?;
                    String::from_utf8_lossy(blob.content()).to_string()
                }
                None => String::new(),
            };
            let theirs = match conflict.their {
                Some(ref entry) => {
                    let blob = self.repo.find_blob(entry.id)?;
                    String::from_utf8_lossy(blob.content()).to_string()
                }
                None => String::new(),
            };

            let ours_hlc = self.find_hlc_for_path(ours_commit, &path);
            let theirs_hlc = self.find_hlc_for_path(theirs_commit, &path);

            conflicts.push(ConflictFile {
                path,
                ancestor,
                ours,
                theirs,
                ours_hlc,
                theirs_hlc,
            });
        }
        Ok(conflicts)
    }

    /// Delete a file, stage the removal, and commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn delete_file(&self, rel_path: &str, message: &str) -> Result<CommitHash> {
        validate_path(&self.path, rel_path)?;
        let full_path = self.path.join(rel_path);
        if full_path.exists() {
            std::fs::remove_file(&full_path)?;
        }
        let mut index = self.repo.index()?;
        index.remove_path(Path::new(rel_path))?;
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;
        let parent = self
            .head_commit()
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        self.write_commit_graph();
        Ok(CommitHash(oid.to_string()))
    }

    /// Delete multiple files, stage the removals, and commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn delete_files(&self, paths: &[&str], message: &str) -> Result<CommitHash> {
        for rel_path in paths {
            validate_path(&self.path, rel_path)?;
        }
        for rel_path in paths {
            let full_path = self.path.join(rel_path);
            if full_path.exists() {
                std::fs::remove_file(&full_path)?;
            }
        }
        let mut index = self.repo.index()?;
        for rel_path in paths {
            index.remove_path(Path::new(rel_path))?;
        }
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;
        let parent = self
            .head_commit()
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        self.write_commit_graph();
        Ok(CommitHash(oid.to_string()))
    }

    /// Rename (move) a file in git: read old content, write to new path, delete old.
    pub fn rename_file(&self, old_path: &str, new_path: &str, message: &str) -> Result<CommitHash> {
        validate_path(&self.path, old_path)?;
        validate_path(&self.path, new_path)?;
        let full_old = self.path.join(old_path);
        let full_new = self.path.join(new_path);
        if !full_old.exists() {
            return Err(ZettelError::NotFound(old_path.to_string()));
        }
        if full_new.exists() {
            return Err(ZettelError::InvalidPath(format!(
                "target path already exists: {new_path}"
            )));
        }
        let content = std::fs::read_to_string(&full_old)?;
        self.commit_batch(&[(new_path, &content)], &[old_path], message)
    }

    /// Write and/or delete multiple files in a single commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_batch(
        &self,
        writes: &[(&str, &str)],
        deletes: &[&str],
        message: &str,
    ) -> Result<CommitHash> {
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
        let mut index = self.repo.index()?;
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
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        self.write_commit_graph();
        Ok(CommitHash(oid.to_string()))
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

        // Stage all
        let mut index = self.repo.index()?;
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
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let oid = self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        self.write_commit_graph();
        Ok(CommitHash(oid.to_string()))
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

    /// Load repository config from `.zetteldb.toml`. Returns defaults for missing fields.
    pub fn load_config(&self) -> Result<RepoConfig> {
        match self.read_file(CONFIG_FILE) {
            Ok(content) => {
                let config: RepoConfig =
                    toml::from_str(&content).map_err(|e| ZettelError::Toml(e.to_string()))?;
                Ok(config)
            }
            Err(_) => Ok(RepoConfig::default()),
        }
    }

    /// Read file content from HEAD tree.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn read_file(&self, rel_path: &str) -> Result<String> {
        validate_path(&self.path, rel_path)?;
        let head = self.repo.head()?.peel_to_commit()?;
        let tree = head.tree()?;
        let entry = tree
            .get_path(Path::new(rel_path))
            .map_err(|_| ZettelError::NotFound(rel_path.to_string()))?;
        let blob = self
            .repo
            .find_blob(entry.id())
            .map_err(|_| ZettelError::NotFound(rel_path.to_string()))?;
        let content =
            std::str::from_utf8(blob.content()).map_err(|e| ZettelError::Parse(e.to_string()))?;
        Ok(content.to_string())
    }

    /// Read multiple files from the HEAD tree in a single sequential pass.
    ///
    /// Resolves HEAD once, then iterates paths against the same tree.
    /// Per-file errors are returned inline (not short-circuited).
    pub fn read_files_batch(&self, paths: &[String]) -> Result<Vec<(String, Result<String>)>> {
        let head = self.repo.head()?.peel_to_commit()?;
        let tree = head.tree()?;

        let results = paths
            .iter()
            .map(|rel_path| {
                let content = (|| -> Result<String> {
                    validate_path(&self.path, rel_path)?;
                    let entry = tree
                        .get_path(Path::new(rel_path))
                        .map_err(|_| ZettelError::NotFound(rel_path.to_string()))?;
                    let blob = self
                        .repo
                        .find_blob(entry.id())
                        .map_err(|_| ZettelError::NotFound(rel_path.to_string()))?;
                    let content = std::str::from_utf8(blob.content())
                        .map_err(|e| ZettelError::Parse(e.to_string()))?;
                    Ok(content.to_string())
                })();
                (rel_path.clone(), content)
            })
            .collect();

        Ok(results)
    }

    /// Walk ancestors of `commit` to find the HLC trailer from the most recent
    /// commit that touched `path`.
    pub fn find_hlc_for_path(&self, commit: &git2::Commit, path: &str) -> Option<crate::hlc::Hlc> {
        const MAX_REVWALK_DEPTH: usize = 100;

        let mut revwalk = self.repo.revwalk().ok()?;
        revwalk.push(commit.id()).ok()?;
        revwalk.set_sorting(git2::Sort::TOPOLOGICAL).ok()?;

        for (depth, oid) in revwalk.flatten().enumerate() {
            if depth >= MAX_REVWALK_DEPTH {
                tracing::warn!(path, depth, "HLC revwalk hit depth limit");
                return None;
            }
            let c = match self.repo.find_commit(oid) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(oid = %oid, error = %e, "skipping bad commit in HLC revwalk");
                    continue;
                }
            };
            let c_tree = match c.tree() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(oid = %oid, error = %e, "skipping commit with bad tree in HLC revwalk");
                    continue;
                }
            };

            let parent_tree = c.parent(0).ok().and_then(|p| p.tree().ok());
            let diff = match self
                .repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&c_tree), None)
            {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(oid = %oid, error = %e, "skipping undiffable commit in HLC revwalk");
                    continue;
                }
            };

            let touches_path = diff.deltas().any(|delta| {
                delta
                    .new_file()
                    .path()
                    .or_else(|| delta.old_file().path())
                    .and_then(|p| p.to_str())
                    .is_some_and(|p| p == path)
            });

            if touches_path {
                return crate::hlc::extract_hlc(c.message().unwrap_or(""));
            }
        }
        None
    }

    /// Diff two commit OIDs, returning changed zettel paths with their change kind.
    pub fn diff_paths(
        &self,
        old_oid: &str,
        new_oid: &str,
    ) -> Result<Vec<(crate::types::DiffKind, String)>> {
        use crate::types::DiffKind;

        let old_commit = self
            .repo
            .find_commit(git2::Oid::from_str(old_oid).map_err(|e| ZettelError::Git(e.to_string()))?)
            .map_err(|e| ZettelError::Git(e.to_string()))?;
        let new_commit = self
            .repo
            .find_commit(git2::Oid::from_str(new_oid).map_err(|e| ZettelError::Git(e.to_string()))?)
            .map_err(|e| ZettelError::Git(e.to_string()))?;

        let old_tree = old_commit.tree()?;
        let new_tree = new_commit.tree()?;

        let diff = self
            .repo
            .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), None)?;

        let mut changes = Vec::new();
        diff.foreach(
            &mut |delta, _| {
                let path = delta
                    .new_file()
                    .path()
                    .or_else(|| delta.old_file().path())
                    .and_then(|p| p.to_str())
                    .map(|s| s.to_string());

                if let Some(path) = path {
                    if path.starts_with("zettelkasten/") && path.ends_with(".md") {
                        let kind = match delta.status() {
                            git2::Delta::Added => Some(DiffKind::Added),
                            git2::Delta::Modified => Some(DiffKind::Modified),
                            git2::Delta::Deleted => Some(DiffKind::Deleted),
                            git2::Delta::Renamed => Some(DiffKind::Modified),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            changes.push((kind, path));
                        }
                    }
                }
                true
            },
            None,
            None,
            None,
        )
        .map_err(|e| ZettelError::Git(e.to_string()))?;

        Ok(changes)
    }

    /// Return the ISO 8601 commit date of the most recent commit that touched `rel_path`.
    ///
    /// Returns `None` if the file has no commit history (e.g. untracked).
    pub fn revision_date(&self, rel_path: &str) -> Result<Option<String>> {
        let head = match self.repo.head() {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let head_commit = head.peel_to_commit()?;

        let mut revwalk = self.repo.revwalk()?;
        revwalk.push(head_commit.id())?;
        revwalk.set_sorting(git2::Sort::TIME)?;

        let target = Path::new(rel_path);

        for oid_result in revwalk {
            let oid = oid_result?;
            let commit = self.repo.find_commit(oid)?;
            let tree = commit.tree()?;

            // Check if the path exists in this commit's tree
            if tree.get_path(target).is_err() {
                continue;
            }

            // For the initial commit (no parents), the file was added here
            if commit.parent_count() == 0 {
                return Ok(Some(format_git_time(&commit)));
            }

            // Check if the path changed compared to parent
            let parent = commit.parent(0)?;
            let parent_tree = parent.tree()?;
            let current_entry = tree.get_path(target).ok();
            let parent_entry = parent_tree.get_path(target).ok();

            let changed = match (current_entry, parent_entry) {
                (Some(c), Some(p)) => c.id() != p.id(),
                (Some(_), None) => true, // file added
                _ => false,
            };

            if changed {
                return Ok(Some(format_git_time(&commit)));
            }
        }

        Ok(None)
    }
}

/// Format a git2 commit time as ISO 8601 (RFC 3339).
fn format_git_time(commit: &git2::Commit) -> String {
    let time = commit.time();
    let offset = chrono::FixedOffset::east_opt(time.offset_minutes() * 60)
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
    chrono::DateTime::from_timestamp(time.seconds(), 0)
        .unwrap_or_default()
        .with_timezone(&offset)
        .to_rfc3339()
}

impl crate::traits::ZettelSource for GitRepo {
    fn list_zettels(&self) -> Result<Vec<String>> {
        self.list_zettels()
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

impl crate::traits::ZettelStore for GitRepo {
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

/// Rename a zettel and rewrite all backlinks pointing to it.
///
/// 1. Moves the file via `rename_file()` (first commit).
/// 2. Finds all zettels linking to the old path or bare ID.
/// 3. Rewrites wikilinks in each backlinking file.
/// 4. Commits all rewritten files (second commit).
/// 5. Detects remaining broken references via `broken_backlinks()` (FR-10a).
pub fn rename_zettel(
    repo: &GitRepo,
    index: &crate::indexer::Index,
    old_path: &str,
    new_path: &str,
) -> Result<RenameReport> {
    // Step 1: move the file
    repo.rename_file(
        old_path,
        new_path,
        &format!("rename: {old_path} → {new_path}"),
    )?;

    // Extract the bare ID from the old path (filename without .md)
    let old_id = Path::new(old_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Step 2: find backlinks for both old path and bare ID
    let mut backlinks = index.backlinking_zettel_paths(old_path)?;
    if !old_id.is_empty() && old_id != old_path {
        let by_id = index.backlinking_zettel_paths(old_id)?;
        for entry in by_id {
            if !backlinks.iter().any(|(id, _)| *id == entry.0) {
                backlinks.push(entry);
            }
        }
    }

    let mut report = RenameReport::default();
    let old_target_for_path = old_path.trim_end_matches(".md");

    // Step 3-4: rewrite backlinks and commit
    if !backlinks.is_empty() {
        let new_target_for_path = new_path.trim_end_matches(".md");

        let mut writes: Vec<(String, String)> = Vec::new();
        for (_source_id, source_path) in &backlinks {
            let content = repo.read_file(source_path)?;
            let mut rewritten = content.clone();

            // Rewrite path-qualified links (without .md, as wikilinks typically omit it)
            rewritten =
                crate::parser::rewrite_links(&rewritten, old_target_for_path, new_target_for_path);

            // Rewrite bare ID links
            if !old_id.is_empty() {
                rewritten = crate::parser::rewrite_links(&rewritten, old_id, new_target_for_path);
            }

            if rewritten != content {
                writes.push((source_path.clone(), rewritten));
                report.updated.push(source_path.clone());
            }
        }

        if !writes.is_empty() {
            let write_refs: Vec<(&str, &str)> = writes
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            repo.commit_files(
                &write_refs,
                &format!("refactor: rewrite links after rename {old_path}"),
            )?;
        }
    }

    // Step 5: detect remaining broken references to old target (runs unconditionally)
    index.rebuild_if_stale(repo)?;
    let old_targets = [old_path, old_target_for_path, old_id];
    report.unresolvable = index
        .broken_backlinks()?
        .into_iter()
        .filter(|(_src, target)| old_targets.contains(&target.as_str()))
        .filter_map(|(src_id, _)| index.resolve_path(&src_id).ok())
        .collect();

    Ok(report)
}

#[cfg(test)]
mod tests {
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
        assert!(dir.path().join("zettelkasten/.gitkeep").exists());
        assert!(dir.path().join("reference/.gitkeep").exists());
        assert!(dir.path().join(".nodes/.gitkeep").exists());
        assert!(dir.path().join(".crdt/temp/.gitkeep").exists());
    }

    #[test]
    fn init_creates_gitignore() {
        let (dir, _repo) = temp_repo();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".zdb/"));
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
        repo.commit_file("zettelkasten/test.md", "hello world", "add test")
            .unwrap();
        let content = repo.read_file("zettelkasten/test.md").unwrap();
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
        repo.commit_files(
            &[("zettelkasten/a.md", "aaa"), ("zettelkasten/b.md", "bbb")],
            "add two files",
        )
        .unwrap();
        assert_eq!(repo.read_file("zettelkasten/a.md").unwrap(), "aaa");
        assert_eq!(repo.read_file("zettelkasten/b.md").unwrap(), "bbb");
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
            .map(|i| format!("zettelkasten/{:014}.md", 20260101000000u64 + i))
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
        repo.commit_file("zettelkasten/a.md", "content a", "add a")
            .unwrap();
        let paths = vec![
            "zettelkasten/a.md".to_string(),
            "zettelkasten/missing.md".to_string(),
        ];
        let batch = repo.read_files_batch(&paths).unwrap();
        assert!(batch[0].1.is_ok());
        assert!(batch[1].1.is_err());
    }

    #[test]
    fn list_zettels_finds_md_files() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("zettelkasten/a.md", "a", "add a").unwrap();
        repo.commit_file("zettelkasten/sub/b.md", "b", "add b")
            .unwrap();
        repo.commit_file("reference/c.md", "c", "add c").unwrap();

        let zettels = repo.list_zettels().unwrap();
        assert_eq!(zettels.len(), 2);
        assert!(zettels.iter().any(|p| p == "zettelkasten/a.md"));
        assert!(zettels.iter().any(|p| p == "zettelkasten/sub/b.md"));
    }

    #[test]
    fn init_creates_version_file() {
        let (dir, _repo) = temp_repo();
        let content = std::fs::read_to_string(dir.path().join(".zetteldb-version")).unwrap();
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
            std::fs::write(dir.path().join(".zetteldb-version"), "999").unwrap();
            let raw_repo = Repository::open(dir.path()).unwrap();
            let sig = Signature::now("zdb", "zdb@local").unwrap();
            let mut index = raw_repo.index().unwrap();
            index.add_path(Path::new(".zetteldb-version")).unwrap();
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
        assert!(dir.path().join(".zetteldb.toml").exists());
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
        repo.commit_file(".zetteldb.toml", custom, "custom config")
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
            std::fs::create_dir_all(dir.path().join("zettelkasten")).unwrap();
            std::fs::write(dir.path().join("zettelkasten/.gitkeep"), "").unwrap();
            let sig = Signature::now("zdb", "zdb@local").unwrap();
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
        let repo_b_raw =
            Repository::clone(bare_dir.path().to_str().unwrap(), dir_b.path()).unwrap();
        drop(repo_b_raw);
        let repo_b = GitRepo::open(dir_b.path()).unwrap();

        (dir_a, repo_a, dir_b, repo_b, bare_dir)
    }

    #[test]
    fn push_and_fetch_cycle() {
        let (_da, repo_a, _db, repo_b, _bare) = setup_two_repos();

        repo_a
            .commit_file("zettelkasten/test.md", "hello", "add test")
            .unwrap();
        repo_a.push("origin", "master").unwrap();

        repo_b.fetch("origin", "master").unwrap();
        let result = repo_b.merge_remote("origin", "master").unwrap();
        assert!(matches!(result, MergeResult::FastForward(_)));

        let content = repo_b.read_file("zettelkasten/test.md").unwrap();
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
            .commit_file("zettelkasten/note.md", "version A", "A edits")
            .unwrap();
        repo_a.push("origin", "master").unwrap();

        repo_b
            .commit_file("zettelkasten/note.md", "version B", "B edits")
            .unwrap();
        repo_b.fetch("origin", "master").unwrap();

        let result = repo_b.merge_remote("origin", "master").unwrap();
        match result {
            MergeResult::Conflicts(conflicts, _theirs_oid) => {
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].path, "zettelkasten/note.md");
                assert!(conflicts[0].ours.contains("version B"));
                assert!(conflicts[0].theirs.contains("version A"));
            }
            other => panic!("expected Conflicts, got {:?}", other),
        }
    }

    #[test]
    fn delete_files_removes_multiple() {
        let (_dir, repo) = temp_repo();
        repo.commit_files(
            &[("zettelkasten/a.md", "aaa"), ("zettelkasten/b.md", "bbb")],
            "add two",
        )
        .unwrap();
        repo.delete_files(&["zettelkasten/a.md", "zettelkasten/b.md"], "remove both")
            .unwrap();
        assert!(repo.read_file("zettelkasten/a.md").is_err());
        assert!(repo.read_file("zettelkasten/b.md").is_err());
    }

    #[test]
    fn commit_batch_writes_and_deletes() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("zettelkasten/old.md", "old content", "add old")
            .unwrap();
        repo.commit_batch(
            &[("zettelkasten/new.md", "new content")],
            &["zettelkasten/old.md"],
            "batch op",
        )
        .unwrap();
        assert_eq!(
            repo.read_file("zettelkasten/new.md").unwrap(),
            "new content"
        );
        assert!(repo.read_file("zettelkasten/old.md").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn symlink_read_rejected() {
        let (dir, repo) = temp_repo();
        repo.commit_file("zettelkasten/real.md", "content", "add")
            .unwrap();
        // Create a symlink on disk pointing to the real file
        let link = dir.path().join("zettelkasten/link.md");
        std::os::unix::fs::symlink(dir.path().join("zettelkasten/real.md"), &link).unwrap();
        let err = repo.read_file("zettelkasten/link.md").unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
    }

    #[test]
    fn dotdot_path_rejected() {
        let (_dir, repo) = temp_repo();
        let err = repo.read_file("zettelkasten/../../etc/passwd").unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
    }

    #[test]
    fn normal_path_accepted() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("zettelkasten/normal.md", "ok", "add")
            .unwrap();
        assert_eq!(repo.read_file("zettelkasten/normal.md").unwrap(), "ok");
    }

    #[test]
    #[cfg(unix)]
    fn symlink_write_rejected() {
        let (dir, repo) = temp_repo();
        repo.commit_file("zettelkasten/real.md", "original", "add")
            .unwrap();
        let link = dir.path().join("zettelkasten/link.md");
        std::os::unix::fs::symlink(dir.path().join("zettelkasten/real.md"), &link).unwrap();
        let err = repo
            .commit_file("zettelkasten/link.md", "hacked", "overwrite")
            .unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
        // Original file unchanged
        assert_eq!(repo.read_file("zettelkasten/real.md").unwrap(), "original");
    }

    #[test]
    fn absolute_path_write_rejected() {
        let (_dir, repo) = temp_repo();
        let err = repo
            .commit_file(native_absolute_path(), "hacked", "write outside repo")
            .unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
    }

    #[test]
    fn absolute_path_read_rejected() {
        let (_dir, repo) = temp_repo();
        let err = repo.read_file(native_absolute_path()).unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
    }

    #[test]
    fn diff_paths_detects_added_modified_deleted() {
        let (_dir, repo) = temp_repo();

        // Create initial zettel and record HEAD
        repo.commit_file(
            "zettelkasten/20240101000000.md",
            "---\ntitle: A\n---\nBody A.",
            "add a",
        )
        .unwrap();
        repo.commit_file(
            "zettelkasten/20240102000000.md",
            "---\ntitle: B\n---\nBody B.",
            "add b",
        )
        .unwrap();
        let old_head = repo.head_oid().unwrap().to_string();

        // Modify one, delete one, add one
        repo.commit_file(
            "zettelkasten/20240101000000.md",
            "---\ntitle: A modified\n---\nBody A modified.",
            "modify a",
        )
        .unwrap();
        repo.delete_file("zettelkasten/20240102000000.md", "delete b")
            .unwrap();
        repo.commit_file(
            "zettelkasten/20240103000000.md",
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
    fn diff_paths_ignores_non_zettel_files() {
        let (_dir, repo) = temp_repo();
        let old_head = repo.head_oid().unwrap().to_string();

        repo.commit_file(
            "zettelkasten/20240101000000.md",
            "---\ntitle: Z\n---\n",
            "add zettel",
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
            .commit_file("zettelkasten/note.md", "version A", &msg_a)
            .unwrap();
        repo_a.push("origin", "master").unwrap();

        let hlc_b = crate::hlc::Hlc {
            wall_ms: 6000,
            counter: 0,
            node: "bbb".into(),
        };
        let msg_b = crate::hlc::append_hlc_trailer("B edits", &hlc_b);
        repo_b
            .commit_file("zettelkasten/note.md", "version B", &msg_b)
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
        let msg = crate::hlc::append_hlc_trailer("add zettel", &hlc);
        repo.commit_file(
            "zettelkasten/20260226120000.md",
            "---\ntitle: test\n---\n",
            &msg,
        )
        .unwrap();
        let head = repo.head_commit().unwrap();
        let result = repo.find_hlc_for_path(&head, "zettelkasten/20260226120000.md");
        assert!(result.is_some());
        assert_eq!(result.unwrap().wall_ms, 1000);
    }

    #[test]
    fn find_hlc_for_path_returns_none_without_trailer() {
        let (_dir, repo) = temp_repo();
        repo.commit_file(
            "zettelkasten/20260226120000.md",
            "---\ntitle: test\n---\n",
            "add zettel",
        )
        .unwrap();
        let head = repo.head_commit().unwrap();
        let result = repo.find_hlc_for_path(&head, "zettelkasten/20260226120000.md");
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
        let msg = crate::hlc::append_hlc_trailer("add zettel", &hlc);
        repo.commit_file("zettelkasten/20260226120000.md", "test", &msg)
            .unwrap();
        let head = repo.head_commit().unwrap();
        let result = repo.find_hlc_for_path(&head, "zettelkasten/99990101000000.md");
        assert!(result.is_none());
    }

    #[test]
    fn rename_file_moves_and_commits() {
        let (dir, repo) = temp_repo();
        repo.commit_file("zettelkasten/20260301120000.md", "hello", "add")
            .unwrap();
        let hash = repo
            .rename_file(
                "zettelkasten/20260301120000.md",
                "zettelkasten/contact/20260301120000.md",
                "rename",
            )
            .unwrap();
        assert!(!hash.0.is_empty());
        assert!(!dir.path().join("zettelkasten/20260301120000.md").exists());
        assert!(dir
            .path()
            .join("zettelkasten/contact/20260301120000.md")
            .exists());
        let content =
            std::fs::read_to_string(dir.path().join("zettelkasten/contact/20260301120000.md"))
                .unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn rename_file_errors_on_missing_source() {
        let (_dir, repo) = temp_repo();
        let err = repo
            .rename_file(
                "zettelkasten/nonexistent.md",
                "zettelkasten/new.md",
                "rename",
            )
            .unwrap_err();
        assert!(matches!(err, ZettelError::NotFound(_)));
    }

    #[test]
    fn rename_file_errors_on_existing_target() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("zettelkasten/a.md", "a", "add a").unwrap();
        repo.commit_file("zettelkasten/b.md", "b", "add b").unwrap();
        let err = repo
            .rename_file("zettelkasten/a.md", "zettelkasten/b.md", "rename")
            .unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
    }

    #[test]
    fn rename_zettel_all_links_resolved() {
        let (dir, repo) = temp_repo();
        let index = crate::indexer::Index::open(&dir.path().join("index.db")).unwrap();

        // Create target zettel A
        let zettel_a = "---\nid: 20260301100000\ntitle: Target\n---\nBody\n";
        repo.commit_file("zettelkasten/20260301100000.md", zettel_a, "add A")
            .unwrap();

        // Create zettel B linking to A via bare ID (matched by backlinking_zettel_paths)
        let zettel_b =
            "---\nid: 20260301110000\ntitle: Linker\n---\nSee [[20260301100000|Target]]\n";
        repo.commit_file("zettelkasten/20260301110000.md", zettel_b, "add B")
            .unwrap();

        // Index both
        let parsed_a = crate::parser::parse(zettel_a, "zettelkasten/20260301100000.md").unwrap();
        let parsed_b = crate::parser::parse(zettel_b, "zettelkasten/20260301110000.md").unwrap();
        index.index_zettel(&parsed_a).unwrap();
        index.index_zettel(&parsed_b).unwrap();

        let report = rename_zettel(
            &repo,
            &index,
            "zettelkasten/20260301100000.md",
            "zettelkasten/contact/20260301100000.md",
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
    fn rename_zettel_no_backlinks_means_empty_report() {
        let (dir, repo) = temp_repo();
        let index = crate::indexer::Index::open(&dir.path().join("index.db")).unwrap();

        // Create a lone zettel with no backlinks
        let zettel_a = "---\nid: 20260301100000\ntitle: Alone\n---\nBody\n";
        repo.commit_file("zettelkasten/20260301100000.md", zettel_a, "add A")
            .unwrap();

        let parsed_a = crate::parser::parse(zettel_a, "zettelkasten/20260301100000.md").unwrap();
        index.index_zettel(&parsed_a).unwrap();

        let report = rename_zettel(
            &repo,
            &index,
            "zettelkasten/20260301100000.md",
            "zettelkasten/contact/20260301100000.md",
        )
        .unwrap();

        assert!(report.updated.is_empty());
        assert!(report.unresolvable.is_empty());
    }

    #[test]
    fn rename_zettel_detects_unresolvable_path_qualified_link() {
        let (dir, repo) = temp_repo();
        let index = crate::indexer::Index::open(&dir.path().join("index.db")).unwrap();

        // Create target zettel A
        let zettel_a = "---\nid: 20260301100000\ntitle: Target\n---\nBody\n";
        repo.commit_file("zettelkasten/20260301100000.md", zettel_a, "add A")
            .unwrap();

        // Create zettel B with path-qualified wikilink (without .md).
        // backlinking_zettel_paths queries by exact target_path, which stores
        // "zettelkasten/20260301100000" — this won't match old_path
        // "zettelkasten/20260301100000.md" or old_id "20260301100000",
        // so the link is never found for rewriting.
        let zettel_b = "---\nid: 20260301110000\ntitle: PathLinker\n---\n\
            See [[zettelkasten/20260301100000|Target]]\n";
        repo.commit_file("zettelkasten/20260301110000.md", zettel_b, "add B")
            .unwrap();

        let parsed_a = crate::parser::parse(zettel_a, "zettelkasten/20260301100000.md").unwrap();
        let parsed_b = crate::parser::parse(zettel_b, "zettelkasten/20260301110000.md").unwrap();
        index.index_zettel(&parsed_a).unwrap();
        index.index_zettel(&parsed_b).unwrap();

        let report = rename_zettel(
            &repo,
            &index,
            "zettelkasten/20260301100000.md",
            "zettelkasten/contact/20260301100000.md",
        )
        .unwrap();

        // B's link was not rewritten (backlinking_zettel_paths missed it),
        // so step 5 should detect it as unresolvable
        assert!(
            !report.unresolvable.is_empty(),
            "expected unresolvable reference from B's path-qualified link"
        );
        assert_eq!(report.unresolvable[0], "zettelkasten/20260301110000.md");
    }

    #[test]
    fn zettel_path_flat() {
        assert_eq!(
            super::zettel_path("20260315120000", Some("contact"), false),
            "zettelkasten/20260315120000.md"
        );
    }

    #[test]
    fn zettel_path_folder() {
        assert_eq!(
            super::zettel_path("20260315120000", Some("contact"), true),
            "zettelkasten/contact/20260315120000.md"
        );
    }

    #[test]
    fn zettel_path_no_type() {
        assert_eq!(
            super::zettel_path("20260315120000", None, true),
            "zettelkasten/20260315120000.md"
        );
    }
}
