use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Oid, Repository, Signature};

use crate::error::{DoogatError, Result};
use crate::types::{CommitHash, ConflictFile, MergeResult, RenameReport, RepoConfig};

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
    }

    /// Write resolved files and create a merge commit with two parents.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary_paths: &[&str],
        message: &str,
        theirs: &CommitHash,
    ) -> Result<CommitHash> {
        for (rel_path, _) in files {
            validate_path(&self.path, rel_path)?;
        }
        for rel_path in binary_paths {
            validate_path(&self.path, rel_path)?;
        }

        let our_commit = self
            .head_commit()
            .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
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
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&ours_tree), Some(&theirs_tree), None)?;
        let resolved_set: std::collections::HashSet<&str> = files
            .iter()
            .map(|(p, _)| *p)
            .chain(binary_paths.iter().copied())
            .collect();
        let mut theirs_only = Vec::new();
        for delta in diff.deltas() {
            let dominated = matches!(delta.status(), git2::Delta::Added | git2::Delta::Modified);
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
        for path in binary_paths {
            index.add_path(Path::new(path))?;
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
            .map_err(|_| DoogatError::NotFound(fetch_head_ref.clone()))?;
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
            .ok_or_else(|| DoogatError::Parse("no HEAD".into()))?;
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

            let ours_blob_oid = conflict.our.as_ref().map(|e| e.id.to_string());
            let theirs_blob_oid = conflict.their.as_ref().map(|e| e.id.to_string());

            let ours_hlc = self.find_hlc_for_path(ours_commit, &path);
            let theirs_hlc = self.find_hlc_for_path(theirs_commit, &path);

            conflicts.push(ConflictFile {
                path,
                ancestor,
                ours,
                theirs,
                ours_hlc,
                theirs_hlc,
                ours_blob_oid,
                theirs_blob_oid,
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
            .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
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
            .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
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
            return Err(DoogatError::NotFound(old_path.to_string()));
        }
        if full_new.exists() {
            return Err(DoogatError::InvalidPath(format!(
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

    /// Load repository config from `.ddb.toml`. Returns defaults for missing fields.
    pub fn load_config(&self) -> Result<RepoConfig> {
        match self.read_file(CONFIG_FILE) {
            Ok(content) => {
                let config: RepoConfig =
                    toml::from_str(&content).map_err(|e| DoogatError::Toml(e.to_string()))?;
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
            .map_err(|_| DoogatError::NotFound(rel_path.to_string()))?;
        let blob = self
            .repo
            .find_blob(entry.id())
            .map_err(|_| DoogatError::NotFound(rel_path.to_string()))?;
        let content =
            std::str::from_utf8(blob.content()).map_err(|e| DoogatError::Parse(e.to_string()))?;
        Ok(content.to_string())
    }

    /// Read raw blob bytes by OID.
    pub fn read_blob(&self, oid_str: &str) -> Result<Vec<u8>> {
        let oid = Oid::from_str(oid_str)?;
        let blob = self.repo.find_blob(oid)?;
        Ok(blob.content().to_vec())
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
                        .map_err(|_| DoogatError::NotFound(rel_path.to_string()))?;
                    let blob = self
                        .repo
                        .find_blob(entry.id())
                        .map_err(|_| DoogatError::NotFound(rel_path.to_string()))?;
                    let content = std::str::from_utf8(blob.content())
                        .map_err(|e| DoogatError::Parse(e.to_string()))?;
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

    /// Diff two commit OIDs, returning changed doogat paths with their change kind.
    pub fn diff_paths(
        &self,
        old_oid: &str,
        new_oid: &str,
    ) -> Result<Vec<(crate::types::DiffKind, String)>> {
        use crate::types::DiffKind;

        let old_commit = self
            .repo
            .find_commit(git2::Oid::from_str(old_oid).map_err(|e| DoogatError::Git(e.to_string()))?)
            .map_err(|e| DoogatError::Git(e.to_string()))?;
        let new_commit = self
            .repo
            .find_commit(git2::Oid::from_str(new_oid).map_err(|e| DoogatError::Git(e.to_string()))?)
            .map_err(|e| DoogatError::Git(e.to_string()))?;

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
                    if path.starts_with("ddb/") && path.ends_with(".md") {
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
        .map_err(|e| DoogatError::Git(e.to_string()))?;

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

impl crate::traits::GitBackend for GitRepo {
    fn repo_path(&self) -> &Path {
        &self.path
    }

    fn load_config(&self) -> Result<RepoConfig> {
        self.load_config()
    }

    fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        self.add_remote(name, url)
    }

    fn fetch(&self, remote: &str, branch: &str) -> Result<()> {
        self.fetch(remote, branch)
    }

    fn push(&self, remote: &str, branch: &str) -> Result<()> {
        self.push(remote, branch)
    }

    fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        self.merge_remote(remote, branch)
    }

    fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary_paths: &[&str],
        message: &str,
        theirs: &CommitHash,
    ) -> Result<CommitHash> {
        self.commit_merge(files, binary_paths, message, theirs)
    }

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

    fn rename_file(&self, old_path: &str, new_path: &str, message: &str) -> Result<CommitHash> {
        self.rename_file(old_path, new_path, message)
    }

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

/// Rename a doogat and rewrite all backlinks pointing to it.
///
/// 1. Moves the file via `rename_file()` (first commit).
/// 2. Finds all doogats linking to the old path or bare ID.
/// 3. Rewrites wikilinks in each backlinking file.
/// 4. Commits all rewritten files (second commit).
/// 5. Detects remaining broken references via `broken_backlinks()` (FR-10a).
pub fn rename_doogat(
    repo: &impl crate::traits::GitBackend,
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
    let mut backlinks = index.backlinking_doogat_paths(old_path)?;
    if !old_id.is_empty() && old_id != old_path {
        let by_id = index.backlinking_doogat_paths(old_id)?;
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
mod tests;
