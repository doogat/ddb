use git2::Oid;

use super::{validate_path, GitRepo};
use crate::error::{DoogatError, Result};
use crate::types::{CommitHash, ConflictFile, MergeResult};

impl GitRepo {
    /// Create a merge commit with two parents from a true three-way merge tree,
    /// overlaying the CRDT-resolved blobs onto the conflict entries.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary: &[(&str, &str)], // (path, winning-blob-OID) — was `binary_paths: &[&str]`
        message: &str,
        theirs: &CommitHash,
    ) -> Result<CommitHash> {
        let hash = self.with_write_lock(|| {
            for (rel_path, _) in files {
                validate_path(&self.path, rel_path)?;
            }
            for (rel_path, _) in binary {
                validate_path(&self.path, rel_path)?;
            }

            let our_commit = self
                .head_commit()
                .ok_or_else(|| DoogatError::Git("repo has no initial commit".into()))?;
            let theirs_oid = Oid::from_str(&theirs.0)?;
            let their_commit = self.repo.find_commit(theirs_oid)?;

            // Reuse the clean path's construction: libgit2 computes the true
            // three-way merge tree (ours-only edits kept, theirs-only kept,
            // both-edited auto-mergeable files line-merged, theirs-deletions
            // absent, ours-creates present). Only the conflict entries need
            // our CRDT-resolved blobs overlaid.
            let mut merge_index = self.repo.merge_commits(&our_commit, &their_commit, None)?;

            // DIVERGENCE GUARD (fail-loud, in-scope per PRD "conflict-set divergence
            // handling"). The re-run merge's conflict set MUST equal the set
            // `sync_manager` resolved (`files` + `binary`). If a concurrent writer
            // moved HEAD in the resolve→commit window, the re-run set differs — a
            // path auto-resolved (no longer a conflict) or a new path conflicts —
            // and our resolved blobs are stale. Checking `has_conflicts()` AFTER the
            // overlay is NOT sufficient: `conflict_remove` returns Ok (not NotFound)
            // for an auto-resolved stage-0 path, so the overlay would silently
            // overwrite libgit2's fresh result and `has_conflicts()` would be false.
            // Snapshot the conflict paths BEFORE the overlay clears them and compare.
            // Because the whole tree is built in-memory (no worktree write until the
            // post-commit checkout), an abort HERE mutates nothing on disk — the
            // binary winners are overlaid by OID, not pre-written (fixes the stale-
            // binary-on-abort window a pre-write would leave).
            let rerun_conflicts = self.collect_conflict_paths(&merge_index)?;
            let resolved: std::collections::HashSet<&str> = files
                .iter()
                .map(|(p, _)| *p)
                .chain(binary.iter().map(|(p, _)| *p))
                .collect();
            if rerun_conflicts.len() != resolved.len()
                || !rerun_conflicts.iter().all(|p| resolved.contains(p.as_str()))
            {
                return Err(DoogatError::Conflict(
                    "merge conflict set changed since resolution (HEAD moved during the \
                     resolve→commit window); retry sync"
                        .into(),
                ));
            }

            self.overlay_resolved_blobs(&mut merge_index, files, binary)?;

            // `write_tree_to` additionally refuses any not-fully-merged index — a
            // second backstop, though the equality guard above already guarantees the
            // overlay clears every conflict.
            let tree_oid = merge_index.write_tree_to(&self.repo)?;
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
            // Sync the working tree to the committed merge tree — symmetric with
            // `perform_normal_merge`. This is what removes theirs-deleted files from
            // the worktree and materializes line-merged and resolved content,
            // replacing the old manual `write_resolved_files`.
            self.repo
                .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            Ok(CommitHash(oid.to_string()))
        })?;
        self.write_commit_graph();
        Ok(hash)
    }

    /// Overlay CRDT-resolved blobs onto a `merge_commits` index's conflict entries.
    /// Text blobs are created from `files` (path, resolved-content); binary-reference
    /// blobs are overlaid by their already-existing winning OID (`binary`: path,
    /// oid-string) — no worktree read/write, so the whole tree is built in memory and
    /// an abort before commit mutates nothing on disk. After this, every path the
    /// resolver produced is a single stage-0 entry; the post-commit `checkout_head`
    /// materializes both text and binary content into the worktree.
    fn overlay_resolved_blobs(
        &self,
        index: &mut git2::Index,
        files: &[(&str, &str)],
        binary: &[(&str, &str)],
    ) -> Result<()> {
        for (path, content) in files {
            let oid = self.repo.blob(content.as_bytes())?;
            Self::resolve_index_entry(index, path, oid)?;
        }
        for (path, oid_str) in binary {
            let oid = git2::Oid::from_str(oid_str)?;
            Self::resolve_index_entry(index, path, oid)?;
        }
        Ok(())
    }

    /// Replace any conflict at `path` with a single stage-0 entry for `oid`.
    /// `conflict_remove` clears the ancestor/ours/theirs stages
    /// (`git_index_conflict_remove` — keeps stage 0, drops stages 1-3);
    /// a path with no conflict returns `ErrorCode::NotFound`, which is fine — the
    /// following `add` installs (or overwrites) the stage-0 entry either way.
    ///
    /// NOTE: `Index::add` alone does NOT clear conflict stages — for a stage-0
    /// insert on a conflicted path, `index_insert` finds no stage-0 existing entry
    /// and appends stage 0 *alongside* stages 1-3, so `write_tree_to` would still
    /// refuse the index. The explicit `conflict_remove` is required.
    fn resolve_index_entry(index: &mut git2::Index, path: &str, oid: git2::Oid) -> Result<()> {
        match index.conflict_remove(std::path::Path::new(path)) {
            Ok(()) => {}
            Err(e) if e.code() == git2::ErrorCode::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        index.add(&Self::stage0_entry(path, oid))?;
        Ok(())
    }

    /// The set of paths that still carry conflict entries in `index`. The path is
    /// taken from the ours side, falling back to theirs (a modify/delete conflict
    /// has one side absent) — matching `extract_single_conflict`'s path logic. Used
    /// by the divergence guard to compare the re-run merge's conflicts against the
    /// set `sync_manager` resolved.
    fn collect_conflict_paths(
        &self,
        index: &git2::Index,
    ) -> Result<std::collections::HashSet<String>> {
        let mut paths = std::collections::HashSet::new();
        for conflict in index.conflicts()? {
            let conflict = conflict?;
            if let Some(p) = conflict
                .our
                .as_ref()
                .or(conflict.their.as_ref())
                .and_then(|e| String::from_utf8(e.path.clone()).ok())
            {
                paths.insert(p);
            }
        }
        Ok(paths)
    }

    /// A stage-0 `IndexEntry` for a regular file blob at `path`. Stat fields are
    /// zeroed; only `id`, `mode`, and `path` affect the written tree.
    fn stage0_entry(path: &str, oid: git2::Oid) -> git2::IndexEntry {
        git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644, // GIT_FILEMODE_BLOB
            uid: 0,
            gid: 0,
            file_size: 0,
            id: oid,
            flags: 0, // stage 0; `Index::add` recomputes the path-length namemask bits
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        }
    }

    /// Perform a normal (non-fast-forward) merge and return the result.
    fn perform_normal_merge(
        &self,
        remote: &str,
        branch: &str,
        annotated: &git2::AnnotatedCommit,
    ) -> Result<MergeResult> {
        let their_commit = self.repo.find_commit(annotated.id())?;
        let our_commit = self
            .head_commit()
            .ok_or_else(|| DoogatError::Parse("no HEAD".into()))?;
        let _ancestor = self.repo.merge_base(our_commit.id(), their_commit.id())?;

        let mut merge_index = self.repo.merge_commits(&our_commit, &their_commit, None)?;

        if merge_index.has_conflicts() {
            let conflicts = self.extract_conflicts(&merge_index, &our_commit, &their_commit)?;
            self.repo.cleanup_state()?;
            return Ok(MergeResult::Conflicts(
                conflicts,
                CommitHash(their_commit.id().to_string()),
            ));
        }

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
        Ok(MergeResult::Clean(CommitHash(oid.to_string())))
    }

    /// Merge a fetched remote branch, returning the merge result.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        let result = self.with_write_lock(|| {
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
                return Ok(MergeResult::FastForward(CommitHash(target_oid.to_string())));
            }

            self.perform_normal_merge(remote, branch, &annotated)
        })?;
        if matches!(result, MergeResult::FastForward(_) | MergeResult::Clean(_)) {
            self.write_commit_graph();
        }
        Ok(result)
    }

    /// Read blob content as a string, or return an empty string if the entry is None.
    fn read_blob_content(&self, entry: Option<&git2::IndexEntry>) -> Result<String> {
        match entry {
            Some(e) => {
                let blob = self.repo.find_blob(e.id)?;
                Ok(String::from_utf8_lossy(blob.content()).to_string())
            }
            None => Ok(String::new()),
        }
    }

    /// Extract a single conflict entry into a `ConflictFile`.
    fn extract_single_conflict(
        &self,
        conflict: &git2::IndexConflict,
        ours_commit: &git2::Commit,
        theirs_commit: &git2::Commit,
    ) -> Result<ConflictFile> {
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
        let ours = self.read_blob_content(conflict.our.as_ref())?;
        let theirs = self.read_blob_content(conflict.their.as_ref())?;

        let ours_blob_oid = conflict.our.as_ref().map(|e| e.id.to_string());
        let theirs_blob_oid = conflict.their.as_ref().map(|e| e.id.to_string());

        let ours_hlc = self.find_hlc_for_path(ours_commit, &path);
        let theirs_hlc = self.find_hlc_for_path(theirs_commit, &path);

        Ok(ConflictFile {
            path,
            ancestor,
            ours,
            theirs,
            ours_hlc,
            theirs_hlc,
            ours_blob_oid,
            theirs_blob_oid,
        })
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
            conflicts.push(self.extract_single_conflict(&conflict, ours_commit, theirs_commit)?);
        }
        Ok(conflicts)
    }
}
