use std::path::Path;

use git2::Oid;

use super::{validate_path, GitRepo};
use crate::error::{DoogatError, Result};
use crate::types::{CommitHash, ConflictFile, MergeResult};

impl GitRepo {
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
}
