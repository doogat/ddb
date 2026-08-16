use git2::Oid;

use super::{validate_path, GitRepo};
use crate::error::{DoogatError, Result};
use crate::types::{CommitHash, ConflictFile, MergeResult};

impl GitRepo {
    /// Create a merge commit with two parents from a true three-way merge tree,
    /// overlaying the CRDT-resolved blobs onto the conflict entries and folding
    /// each collision loser's reassigned content into the SAME commit (PRD 00167
    /// — never a second commit for losers).
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary: &[(&str, &str)], // (path, winning-blob-OID)
        losers: &[crate::types::CollisionLoser],
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
            self.fold_losers_into_index(&mut merge_index, losers, &our_commit, &their_commit)?;

            // `write_tree_to` additionally refuses any not-fully-merged index — a
            // second backstop, though the equality guard above already guarantees the
            // overlay clears every conflict.
            let tree_oid = merge_index.write_tree_to(&self.repo)?;
            let tree = self.repo.find_tree(tree_oid)?;
            if let Some(theirs_hlc) = crate::hlc::extract_hlc(their_commit.message().unwrap_or("")) {
                self.hlc_clock.recv(&theirs_hlc);
            }
            let oid = self.create_commit(message, &tree, &[&our_commit, &their_commit])?;
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

    /// Fold each collision loser's reassigned content (new deterministic ID,
    /// rewritten frontmatter, rewritten inbound links) into `merge_index`, inside
    /// the same merge commit as the winner. Losers are processed sequentially so
    /// each loser's id derivation sees every prior loser's already-taken id —
    /// this is what makes two losers in the same call land at distinct paths
    /// even if their computed candidates are close together.
    ///
    /// Atomic-abort contract: a loser that fails to fold (e.g. `rewrite_id_field`
    /// finds no frontmatter block to rewrite) aborts this call via `?`, which
    /// aborts `commit_merge` before its single `create_commit` — nothing lands,
    /// not the winner, not an earlier loser in the same batch. Per-loser
    /// degradation is deliberately not offered here: landing the winner while
    /// silently dropping a loser is exactly the half-resolved data-loss shape
    /// PRD 00167 removed. The repo's "one bad file never fails the batch" P0
    /// invariant scopes to read/list/index paths, not to this git write path.
    fn fold_losers_into_index(
        &self,
        merge_index: &mut git2::Index,
        losers: &[crate::types::CollisionLoser],
        our_commit: &git2::Commit,
        their_commit: &git2::Commit,
    ) -> Result<()> {
        let mut taken_ids = Self::collect_taken_ids(merge_index);
        for loser in losers {
            let winner_commit = if loser.theirs_won {
                their_commit
            } else {
                our_commit
            };
            self.fold_one_loser(merge_index, loser, winner_commit, &mut taken_ids)?;
        }
        Ok(())
    }

    /// Snapshot every doogat id already present anywhere under `ddb/` -- other
    /// type folders and `ddb/_typedef/` included, neither of which
    /// `doogat_path` can even address -- once, up front. An id is taken
    /// repo-wide, not just at the loser's own flat/type-folder path. Kept
    /// current as each loser lands (mirrors `id_minting::existence_oracle`'s
    /// snapshot-then-answer idiom, but against the in-memory merge tree
    /// instead of HEAD) so a later loser in the same batch still sees an
    /// earlier loser's just-assigned id as taken.
    fn collect_taken_ids(merge_index: &git2::Index) -> std::collections::HashSet<String> {
        merge_index
            .iter()
            .filter_map(|entry| {
                let path = String::from_utf8(entry.path.clone()).ok()?;
                if !path.starts_with("ddb/") || !path.ends_with(".md") {
                    return None;
                }
                crate::parser::extract_id_from_path(&path)
            })
            .collect()
    }

    /// Fold one collision loser's reassigned content (new deterministic ID,
    /// rewritten frontmatter, rewritten inbound links) into `merge_index`.
    /// `taken_ids` is threaded through by mutable reference and updated with
    /// the newly derived id before returning, so the next loser processed in
    /// the same `fold_losers_into_index` call sees it as taken.
    fn fold_one_loser(
        &self,
        merge_index: &mut git2::Index,
        loser: &crate::types::CollisionLoser,
        winner_commit: &git2::Commit,
        taken_ids: &mut std::collections::HashSet<String>,
    ) -> Result<()> {
        let exists = |candidate: &str| -> bool { taken_ids.contains(candidate) };
        let new_id =
            crate::id_minting::derive_content_id(&loser.old_id, &loser.losing_blob_oid, exists);
        taken_ids.insert(new_id.0.clone());
        let new_content =
            crate::parser::rewrite_id_field(&loser.content, &new_id.0).map_err(|e| {
                DoogatError::Conflict(format!(
                    "collision loser at {} (old id {}) could not be rewritten: {e}",
                    loser.old_path, loser.old_id
                ))
            })?;
        let new_path =
            crate::git_ops::doogat_path(&new_id.0, loser.type_name.as_deref(), loser.folder);

        tracing::warn!(
            old_id = %loser.old_id,
            new_id = %new_id.0,
            old_path = %loser.old_path,
            new_path = %new_path,
            "collision resolved: doogat ID reassigned"
        );

        let old_path_no_ext = loser
            .old_path
            .strip_suffix(".md")
            .unwrap_or(&loser.old_path);
        let new_path_no_ext = new_path.strip_suffix(".md").unwrap_or(&new_path);

        let rewritten_links = self.scan_and_rewrite_links_in_index(
            merge_index,
            winner_commit,
            &loser.old_id,
            old_path_no_ext,
            &new_id.0,
            new_path_no_ext,
        )?;

        let oid = self.repo.blob(new_content.as_bytes())?;
        Self::resolve_index_entry(merge_index, &new_path, oid)?;

        for (path, content) in rewritten_links {
            let oid = self.repo.blob(content.as_bytes())?;
            Self::resolve_index_entry(merge_index, &path, oid)?;
        }
        Ok(())
    }

    /// Scan every `ddb/*.md` entry in `index` for inline references to `old_id`
    /// (frontmatter id, ID-form links, or path-form links), skipping any
    /// reference that also appears in `winner_commit`'s tree at the same path —
    /// that reference was written against the doogat that KEPT `old_id`, not the
    /// loser, so it must not be rewritten. `winner_commit` is whichever side
    /// `lww_pick` chose (`CollisionLoser::theirs_won`); since PRD 00166 ours can
    /// win too, keying this on theirs would corrupt the winner's own backlinks.
    /// Does not write to `index`; returns `(path, rewritten_content)` pairs for
    /// `fold_one_loser` to overlay.
    fn scan_and_rewrite_links_in_index(
        &self,
        index: &git2::Index,
        winner_commit: &git2::Commit,
        old_id: &str,
        old_path_no_ext: &str,
        new_id: &str,
        new_path_no_ext: &str,
    ) -> Result<Vec<(String, String)>> {
        let mut rewritten = Vec::new();
        for entry in index.iter() {
            let Ok(entry_path) = String::from_utf8(entry.path.clone()) else {
                continue;
            };
            if !entry_path.starts_with("ddb/") || !entry_path.ends_with(".md") {
                continue;
            }
            let blob = self.repo.find_blob(entry.id)?;
            let content = match std::str::from_utf8(blob.content()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        path = %entry_path,
                        error = %e,
                        "skipping non-UTF-8 blob in collision link-rewrite scan"
                    );
                    continue;
                }
            };
            if !content.contains(old_id) {
                continue;
            }

            if self.winner_tree_mentions_id(winner_commit, &entry_path, old_id)? {
                continue;
            }

            let by_id = crate::parser::rewrite_links(content, old_id, new_id);
            let rewritten_content =
                crate::parser::rewrite_links(&by_id, old_path_no_ext, new_path_no_ext);
            if rewritten_content != content {
                rewritten.push((entry_path, rewritten_content));
            }
        }
        Ok(rewritten)
    }

    /// True when `winner_commit`'s tree holds `entry_path` and that blob also
    /// mentions `old_id`: the reference was written against the doogat that KEPT
    /// `old_id`, so the loser's rewrite must leave it alone. A path absent from
    /// the winner's tree is not a winner reference, hence `false`.
    fn winner_tree_mentions_id(
        &self,
        winner_commit: &git2::Commit,
        entry_path: &str,
        old_id: &str,
    ) -> Result<bool> {
        let Ok(winner_entry) = winner_commit
            .tree()?
            .get_path(std::path::Path::new(entry_path))
        else {
            return Ok(false);
        };
        let winner_blob = self.repo.find_blob(winner_entry.id())?;
        Ok(String::from_utf8_lossy(winner_blob.content()).contains(old_id))
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
        allow_unrelated: bool,
    ) -> Result<MergeResult> {
        let their_commit = self.repo.find_commit(annotated.id())?;
        let our_commit = self
            .head_commit()
            .ok_or_else(|| DoogatError::Parse("no HEAD".into()))?;
        // The unrelated-histories guard is skipped only when the caller
        // explicitly opts in via `merge_remote_allowing_unrelated` (bundle
        // import: a fresh repo importing an established bundle has no common
        // ancestor by design). Ordinary `merge_remote` always enforces it: a
        // sync merge with no common ancestor is almost always a
        // misconfigured remote, and should keep failing loudly instead of
        // silently unioning two unrelated trees.
        if !allow_unrelated {
            self.repo.merge_base(our_commit.id(), their_commit.id())?;
        }

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
        if let Some(theirs_hlc) = crate::hlc::extract_hlc(their_commit.message().unwrap_or("")) {
            self.hlc_clock.recv(&theirs_hlc);
        }
        let oid =
            self.create_commit(&format!("merge {remote}/{branch}"), &tree, &[&our_commit, &their_commit])?;
        self.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
        Ok(MergeResult::Clean(CommitHash(oid.to_string())))
    }

    /// Merge a fetched remote branch, returning the merge result. Always
    /// enforces the unrelated-histories guard.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        self.merge_remote_with_policy(remote, branch, false)
    }

    /// Merge a fetched remote branch whose history is expected to share no
    /// common ancestor with the local one (bundle import). Identical to
    /// `merge_remote` except that the unrelated-histories guard is not applied.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn merge_remote_allowing_unrelated(
        &self,
        remote: &str,
        branch: &str,
    ) -> Result<MergeResult> {
        self.merge_remote_with_policy(remote, branch, true)
    }

    /// Shared implementation behind `merge_remote` and
    /// `merge_remote_allowing_unrelated`; `allow_unrelated` is the only
    /// difference in behavior between the two.
    fn merge_remote_with_policy(
        &self,
        remote: &str,
        branch: &str,
        allow_unrelated: bool,
    ) -> Result<MergeResult> {
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
                if let Ok(their_commit) = self.repo.find_commit(target_oid) {
                    if let Some(theirs_hlc) =
                        crate::hlc::extract_hlc(their_commit.message().unwrap_or(""))
                    {
                        self.hlc_clock.recv(&theirs_hlc);
                    }
                }
                return Ok(MergeResult::FastForward(CommitHash(target_oid.to_string())));
            }

            self.perform_normal_merge(remote, branch, &annotated, allow_unrelated)
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
