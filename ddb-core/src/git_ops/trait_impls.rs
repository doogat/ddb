use std::path::Path;
use std::time::Duration;

use git2::Oid;

use crate::error::{DoogatError, Result};
use crate::types::{CommitHash, MergeResult, RepoConfig};

use super::GitRepo;

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

    fn fetch(&self, remote: &str, branch: &str, timeout: Duration) -> Result<()> {
        self.fetch(remote, branch, timeout)
    }

    fn push(&self, remote: &str, branch: &str, timeout: Duration) -> Result<()> {
        self.push(remote, branch, timeout)
    }

    fn delete_remote_ref(&self, remote: &str, branch: &str) -> Result<()> {
        self.delete_remote_ref(remote, branch)
    }
}

impl crate::traits::GitMerge for GitRepo {
    fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        self.merge_remote(remote, branch)
    }

    fn merge_remote_allowing_unrelated(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        self.merge_remote_allowing_unrelated(remote, branch)
    }

    fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary: &[(&str, &str)],
        losers: &[crate::types::CollisionLoser],
        message: &str,
        theirs: &CommitHash,
    ) -> Result<crate::types::MergeCommitOutcome> {
        self.commit_merge(files, binary, losers, message, theirs)
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

    fn with_write_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.with_write_lock(f)
    }
}
