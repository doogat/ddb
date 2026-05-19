use crate::crdt_resolver;
use crate::error::{DoogatError, Result};
use crate::git_ops::GitRepo;
use crate::hlc::Hlc;
use crate::indexer::Index;
use crate::parser;
use crate::traits::GitBackend;
use crate::types::{CommitHash, ConflictFile, MergeResult, NodeConfig, SyncReport};

impl From<toml::de::Error> for DoogatError {
    fn from(e: toml::de::Error) -> Self {
        Self::Toml(e.to_string())
    }
}

pub struct SyncManager<'a, G: GitBackend = GitRepo> {
    pub repo: &'a G,
    pub node: NodeConfig,
}

/// Register a new sync node for the given repo.
pub fn register_node(repo: &impl GitBackend, name: &str) -> Result<NodeConfig> {
    let uuid = uuid::Uuid::new_v4().to_string();

    let node = NodeConfig {
        uuid: uuid.clone(),
        name: name.to_string(),
        known_heads: Vec::new(),
        last_sync: None,
        hlc: None,
        status: crate::types::NodeStatus::Active,
        created: Some(chrono::Utc::now().to_rfc3339()),
    };

    // Write .nodes/{uuid}.toml
    let toml_content =
        toml::to_string_pretty(&node).map_err(|e| DoogatError::Parse(e.to_string()))?;
    let node_path = format!(".nodes/{uuid}.toml");
    repo.commit_file(&node_path, &toml_content, &format!("register node {name}"))?;

    // Store UUID locally (not tracked by git)
    let local_path = repo.repo_path().join(".git/ddb-node");
    std::fs::write(local_path, &uuid)?;

    Ok(node)
}

/// Add `resurrected: true` to the frontmatter of a surviving file in a delete-vs-edit conflict.
fn add_resurrected_marker(content: &str) -> String {
    if let Ok(zones) = parser::split_zones(content) {
        let fm = if zones.raw_frontmatter.contains("resurrected:") {
            zones.raw_frontmatter.clone()
        } else {
            format!("{}\nresurrected: true", zones.raw_frontmatter.trim_end())
        };
        // Reassemble
        if zones.reference_section.is_empty() {
            format!("---\n{fm}\n---\n{}", zones.body)
        } else {
            format!(
                "---\n{fm}\n---\n{}\n---\n{}",
                zones.body, zones.reference_section
            )
        }
    } else {
        // Can't parse — return as-is
        content.to_string()
    }
}

/// Write `_fm.crdt` files for resolved files that carry frontmatter CRDT state.
fn write_fm_crdt_files(
    repo_path: &std::path::Path,
    commit_hash: &CommitHash,
    resolved: &[crate::types::ResolvedFile],
) -> Result<()> {
    let temp_dir = repo_path.join(".crdt/temp");
    for r in resolved {
        if let Some(bytes) = &r.fm_crdt_bytes {
            let doogat_id = std::path::Path::new(&r.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            std::fs::create_dir_all(&temp_dir)?;
            let name = format!("{}_{doogat_id}_fm.crdt", commit_hash.0);
            std::fs::write(temp_dir.join(name), bytes)?;
        }
    }
    Ok(())
}

/// Info stashed during collision resolution for post-merge loser reassignment.
struct CollisionLoser {
    old_id: String,
    old_path: String,
    content: String,
    folder: bool,
    type_name: Option<String>,
}

/// Partition conflicts into four buckets: binary references, delete-vs-edit,
/// add-add collisions, and normal (content) conflicts.
fn partition_conflicts(
    conflicts: Vec<ConflictFile>,
) -> (
    Vec<ConflictFile>,
    Vec<ConflictFile>,
    Vec<ConflictFile>,
    Vec<ConflictFile>,
) {
    let mut binary_ref = Vec::new();
    let mut delete_edit = Vec::new();
    let mut add_add = Vec::new();
    let mut normal = Vec::new();
    for c in conflicts {
        if c.ours.is_empty() || c.theirs.is_empty() {
            delete_edit.push(c);
        } else if c.path.starts_with("reference/") {
            binary_ref.push(c);
        } else if c.ancestor.is_none() {
            add_add.push(c);
        } else {
            normal.push(c);
        }
    }
    (binary_ref, delete_edit, add_add, normal)
}

/// Pick winner from an add-add collision. Later HLC wins; theirs on tie/missing.
fn resolve_add_add_collision(
    conflict: &ConflictFile,
) -> (crate::types::ResolvedFile, CollisionLoser) {
    let theirs_wins = match (&conflict.ours_hlc, &conflict.theirs_hlc) {
        (Some(ours_hlc), Some(theirs_hlc)) => theirs_hlc >= ours_hlc,
        _ => true,
    };

    let (winner_content, loser_content) = if theirs_wins {
        (conflict.theirs.clone(), conflict.ours.clone())
    } else {
        (conflict.ours.clone(), conflict.theirs.clone())
    };

    let old_id = std::path::Path::new(&conflict.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    // Infer folder mode from path depth: ddb/{type}/{id}.md = 3 parts
    let parts: Vec<&str> = conflict.path.split('/').collect();
    let folder = parts.len() > 2;
    let type_name = if folder {
        parts.get(1).map(|s| s.to_string())
    } else {
        None
    };

    let resolved = crate::types::ResolvedFile {
        path: conflict.path.clone(),
        content: winner_content,
        fm_crdt_bytes: None,
    };

    let loser = CollisionLoser {
        old_id,
        old_path: conflict.path.clone(),
        content: loser_content,
        folder,
        type_name,
    };

    (resolved, loser)
}

/// Update the `id` field in a doogat's frontmatter.
fn update_frontmatter_id(content: &str, new_id: &str) -> Result<String> {
    let mut parsed = parser::parse(content, "collision-loser")?;
    parsed.meta.id = Some(crate::types::DoogatId(new_id.to_string()));
    Ok(parser::serialize(&parsed))
}

/// Ensures `skip_commit_graph` is reset when sync exits (success or error).
struct SkipCommitGraphResetGuard<'a, G: GitBackend> {
    repo: &'a G,
}

impl<G: GitBackend> Drop for SkipCommitGraphResetGuard<'_, G> {
    fn drop(&mut self) {
        self.repo.set_skip_commit_graph(false);
    }
}

impl<'a, G: GitBackend> SyncManager<'a, G> {
    /// Open a SyncManager from an existing repo with a registered node.
    pub fn open(repo: &'a G) -> Result<Self> {
        let local_path = repo.repo_path().join(".git/ddb-node");
        let uuid = std::fs::read_to_string(&local_path)
            .map_err(|_| DoogatError::NotFound("no node registered (.git/ddb-node)".into()))?;
        let uuid = uuid.trim().to_string();

        let node_path = format!(".nodes/{uuid}.toml");
        let toml_content = repo.read_file(&node_path)?;
        let node: NodeConfig = toml::from_str(&toml_content)?;

        Ok(Self { repo, node })
    }

    /// List all registered nodes.
    pub fn list_nodes(&self) -> Result<Vec<NodeConfig>> {
        let head_oid = self.repo.head_oid()?;
        let entries = self.repo.walk_tree_files(&head_oid.0, ".nodes/")?;
        let mut nodes = Vec::new();
        for (path, content) in &entries {
            if path.ends_with(".toml") {
                match toml::from_str::<NodeConfig>(content) {
                    Ok(node) => nodes.push(node),
                    Err(e) => tracing::warn!("failed to parse node config {path}: {e}"),
                }
            }
        }
        Ok(nodes)
    }

    /// Full sync cycle: fetch → merge → resolve → push → update state → reindex.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn sync(&mut self, remote: &str, branch: &str, index: &Index) -> Result<SyncReport> {
        let sync_start = std::time::Instant::now();
        tracing::info!(remote, branch, "sync_start");

        // Defer commit-graph writes until end of sync
        self.repo.set_skip_commit_graph(true);
        let _reset_skip_commit_graph = SkipCommitGraphResetGuard { repo: self.repo };

        // Fetch
        let phase_start = std::time::Instant::now();
        self.repo.fetch(remote, branch)?;
        tracing::info!(
            phase = "fetch",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        // Merge
        let phase_start = std::time::Instant::now();
        let merge_result = self.repo.merge_remote(remote, branch)?;
        let mut report = self.apply_merge_result(merge_result, index)?;
        tracing::info!(
            phase = "merge",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        self.finalize_sync(remote, branch, index, &mut report)?;

        tracing::info!(total_ms = sync_start.elapsed().as_millis(), "sync_complete");
        Ok(report)
    }

    /// Dispatch on merge result, resolving conflicts if needed.
    fn apply_merge_result(
        &mut self,
        merge_result: MergeResult,
        index: &Index,
    ) -> Result<SyncReport> {
        let mut report = SyncReport {
            direction: "bidirectional".into(),
            commits_transferred: 0,
            conflicts_resolved: 0,
            resurrected: 0,
            collisions_reassigned: 0,
            singleton_conflicts_resolved: 0,
            singleton_conflicts: Vec::new(),
        };

        match merge_result {
            MergeResult::AlreadyUpToDate => {
                tracing::info!("merge_result: up-to-date");
                report.direction = "up-to-date".into();
            }
            MergeResult::FastForward(_) => {
                report.commits_transferred = 1;
            }
            MergeResult::Clean(oid) => {
                report.commits_transferred = 1;
                report.conflicts_resolved = self.validate_clean_merge_or_fallback(oid, index)?;
            }
            MergeResult::Conflicts(conflicts, theirs_oid) => {
                self.resolve_merge_conflicts(conflicts, &theirs_oid, index, &mut report)?;
            }
        }

        Ok(report)
    }

    /// Resolve all conflicts from a merge with conflict markers.
    fn resolve_merge_conflicts(
        &mut self,
        conflicts: Vec<ConflictFile>,
        theirs_oid: &CommitHash,
        index: &Index,
        report: &mut SyncReport,
    ) -> Result<()> {
        let count = conflicts.len();
        tracing::info!(count, "merge_result: conflicts");

        let (binary_ref, delete_edit, add_add, normal) = partition_conflicts(conflicts);

        let mut resolved = Self::resolve_delete_edit_conflicts(&delete_edit);
        let mut collision_losers: Vec<CollisionLoser> = Vec::new();

        report.resurrected = delete_edit.len();
        if report.resurrected > 0 {
            tracing::info!(count = report.resurrected, "delete_edit_resolved");
        }

        for conflict in &add_add {
            let (winner, loser) = resolve_add_add_collision(conflict);
            resolved.push(winner);
            collision_losers.push(loser);
        }

        if !normal.is_empty() {
            let strategy = self.lookup_crdt_strategy_for_conflicts(&normal, index);
            resolved.extend(self.cascade_resolve(normal, strategy.as_deref()));
        }

        self.resolve_binary_ref_conflicts(&binary_ref)?;
        self.create_conflict_commit(&resolved, &binary_ref, theirs_oid)?;

        report.collisions_reassigned =
            self.reassign_collision_losers(collision_losers, theirs_oid)?;
        report.conflicts_resolved = count;
        report.commits_transferred = 1;

        Ok(())
    }

    /// Tick HLC, commit resolved files, and write FM CRDT state.
    fn create_conflict_commit(
        &mut self,
        resolved: &[crate::types::ResolvedFile],
        binary_ref: &[ConflictFile],
        theirs_oid: &CommitHash,
    ) -> Result<()> {
        let hlc = self.tick_hlc();
        let merge_msg = crate::hlc::append_hlc_trailer("resolve merge conflicts via CRDT", &hlc);

        let files: Vec<(&str, &str)> = resolved
            .iter()
            .map(|r| (r.path.as_str(), r.content.as_str()))
            .collect();
        let binary_paths: Vec<&str> = binary_ref.iter().map(|c| c.path.as_str()).collect();
        self.repo
            .commit_merge(&files, &binary_paths, &merge_msg, theirs_oid)?;

        let commit_oid = self.repo.head_oid()?;
        write_fm_crdt_files(self.repo.repo_path(), &commit_oid, resolved)?;
        Ok(())
    }

    /// Post-merge: update sync state, push, commit-graph, reindex.
    fn finalize_sync(
        &mut self,
        remote: &str,
        branch: &str,
        index: &Index,
        report: &mut SyncReport,
    ) -> Result<()> {
        let phase_start = std::time::Instant::now();
        self.update_sync_state()?;
        tracing::info!(
            phase = "update_sync_state",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        let phase_start = std::time::Instant::now();
        let sweep_hlc = self.tick_hlc();
        let sweep =
            crate::consistency::singleton_sweep::singleton_sweep(self.repo, index, &sweep_hlc)?;
        report.singleton_conflicts_resolved = sweep.conflicts_resolved;
        // PRD 00139 cycle-3 #4: surface the per-conflict detail alongside
        // the count. `sweep.details` is `(table, winner, losers)` per
        // resolved conflict, exactly the shape SyncReport now exposes.
        report.singleton_conflicts = sweep
            .details
            .iter()
            .map(
                |(table, winner, losers)| crate::types::SingletonConflictResolution {
                    table: table.clone(),
                    winner: winner.clone(),
                    losers: losers.clone(),
                },
            )
            .collect();
        tracing::info!(
            phase = "singleton_sweep",
            resolved = sweep.conflicts_resolved,
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        let phase_start = std::time::Instant::now();
        self.repo.push(remote, branch)?;
        tracing::info!(
            phase = "push",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        self.repo.set_skip_commit_graph(false);
        self.repo.write_commit_graph();

        let phase_start = std::time::Instant::now();
        index.rebuild_if_stale(self.repo)?;
        tracing::info!(
            phase = "reindex",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        crate::maintenance::maybe_auto_run(self.repo);
        Ok(())
    }

    /// Resolve delete-vs-edit conflicts: the edit wins, a "resurrected" marker is added.
    fn resolve_delete_edit_conflicts(
        conflicts: &[ConflictFile],
    ) -> Vec<crate::types::ResolvedFile> {
        conflicts
            .iter()
            .map(|conflict| {
                let surviving = if conflict.ours.is_empty() {
                    &conflict.theirs
                } else {
                    &conflict.ours
                };
                let deleted_by = if conflict.ours.is_empty() {
                    "local"
                } else {
                    "remote"
                };
                tracing::warn!(
                    doogat_path = %conflict.path,
                    deleted_by,
                    "delete-vs-edit conflict resolved: doogat resurrected"
                );
                crate::types::ResolvedFile {
                    path: conflict.path.clone(),
                    content: add_resurrected_marker(surviving),
                    fm_crdt_bytes: None,
                }
            })
            .collect()
    }

    /// Resolve binary reference conflicts via HLC last-write-wins.
    fn resolve_binary_ref_conflicts(&self, conflicts: &[ConflictFile]) -> Result<()> {
        for conflict in conflicts {
            let theirs_wins = match (&conflict.ours_hlc, &conflict.theirs_hlc) {
                (Some(ours_hlc), Some(theirs_hlc)) => theirs_hlc >= ours_hlc,
                _ => true,
            };
            let winner = if theirs_wins { "theirs" } else { "ours" };
            let winner_oid = if theirs_wins {
                &conflict.theirs_blob_oid
            } else {
                &conflict.ours_blob_oid
            };

            let oid = winner_oid.as_deref().ok_or_else(|| {
                DoogatError::Conflict(format!(
                    "binary conflict at {} missing blob OID for winner ({})",
                    conflict.path, winner
                ))
            })?;
            let bytes = self.repo.read_blob(oid)?;
            let full_path = self.repo.repo_path().join(&conflict.path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, &bytes)?;
            tracing::info!(path = %conflict.path, winner, "binary_lww_resolved");
        }
        Ok(())
    }

    /// Three-step merge cascade:
    /// Step 1 (git merge) already happened. This handles Steps 2+3.
    /// Step 2: CRDT resolve (using typedef strategy or repo default).
    ///   → validate result (parser::parse)
    ///   → if invalid or error → Step 3
    /// Step 3: LWW by HLC (whole-file, always produces valid file).
    fn cascade_resolve(
        &self,
        conflicts: Vec<ConflictFile>,
        strategy: Option<&str>,
    ) -> Vec<crate::types::ResolvedFile> {
        // Step 2: CRDT
        tracing::debug!(
            strategy = strategy.unwrap_or("preset:default"),
            "cascade_step2_crdt"
        );
        match crdt_resolver::resolve_conflicts(conflicts.clone(), strategy) {
            Ok(resolved) => {
                // Validate each resolved file
                let all_valid = resolved
                    .iter()
                    .all(|r| parser::parse(&r.content, &r.path).is_ok());
                if all_valid {
                    return resolved;
                }
                tracing::warn!("CRDT resolution produced invalid output; falling back to LWW");
            }
            Err(e) => {
                tracing::warn!("CRDT resolution failed ({}); falling back to LWW", e);
            }
        }

        // Step 3: LWW by HLC
        match crdt_resolver::resolve_lww(conflicts.clone()) {
            Ok(resolved) => resolved,
            Err(_) => {
                // LWW should never fail, but if it does, ours-wins is the last resort
                conflicts
                    .into_iter()
                    .map(|c| crate::types::ResolvedFile {
                        path: c.path,
                        content: c.ours,
                        fm_crdt_bytes: None,
                    })
                    .collect()
            }
        }
    }

    fn validate_clean_merge_or_fallback(
        &self,
        merge_hash: CommitHash,
        index: &Index,
    ) -> Result<usize> {
        let merge_oid_str = &merge_hash.0;
        if self.repo.commit_parent_count(merge_oid_str)? < 2 {
            return Ok(0);
        }

        let ours_oid = self.repo.commit_parent_oid(merge_oid_str, 0)?;
        let theirs_oid = self.repo.commit_parent_oid(merge_oid_str, 1)?;

        let affected = self.affected_markdown_files(&ours_oid, &theirs_oid, merge_oid_str)?;
        if affected.is_empty() {
            return Ok(0);
        }

        let has_parse_failure = affected.iter().any(|path| {
            self.read_file_from_commit(merge_oid_str, path)
                .map(|content| parser::parse(&content, path).is_err())
                .unwrap_or(false)
        });
        if !has_parse_failure {
            return Ok(0);
        }

        let ancestor_oid = self.repo.merge_base(&ours_oid, &theirs_oid).ok();
        self.crdt_fallback_for_affected(&affected, &ours_oid, &theirs_oid, &ancestor_oid, index)
    }

    /// Re-resolve affected files via CRDT cascade and commit the result.
    fn crdt_fallback_for_affected(
        &self,
        affected: &[String],
        ours_oid: &str,
        theirs_oid: &str,
        ancestor_oid: &Option<String>,
        index: &Index,
    ) -> Result<usize> {
        let conflicts: Vec<ConflictFile> = affected
            .iter()
            .map(|path| ConflictFile {
                path: path.clone(),
                ancestor: ancestor_oid
                    .as_ref()
                    .and_then(|oid| self.read_file_from_commit(oid, path)),
                ours: self
                    .read_file_from_commit(ours_oid, path)
                    .unwrap_or_default(),
                theirs: self
                    .read_file_from_commit(theirs_oid, path)
                    .unwrap_or_default(),
                ours_hlc: self.repo.find_hlc_for_path(ours_oid, path),
                theirs_hlc: self.repo.find_hlc_for_path(theirs_oid, path),
                ours_blob_oid: None,
                theirs_blob_oid: None,
            })
            .collect();

        let strategy = self.lookup_crdt_strategy_for_conflicts(&conflicts, index);
        let resolved = self.cascade_resolve(conflicts, strategy.as_deref());
        let files: Vec<(&str, &str)> = resolved
            .iter()
            .map(|r| (r.path.as_str(), r.content.as_str()))
            .collect();
        self.repo
            .commit_files(&files, "validate clean merge fallback via CRDT")?;

        let commit_oid = self.repo.head_oid()?;
        write_fm_crdt_files(self.repo.repo_path(), &commit_oid, &resolved)?;

        Ok(files.len())
    }

    fn affected_markdown_files(
        &self,
        ours_oid: &str,
        theirs_oid: &str,
        merged_oid: &str,
    ) -> Result<Vec<String>> {
        let mut paths = std::collections::BTreeSet::new();
        for (_, path) in self.repo.diff_paths(ours_oid, merged_oid)? {
            if path.starts_with("ddb/") && path.ends_with(".md") {
                paths.insert(path);
            }
        }
        for (_, path) in self.repo.diff_paths(theirs_oid, merged_oid)? {
            if path.starts_with("ddb/") && path.ends_with(".md") {
                paths.insert(path);
            }
        }
        Ok(paths.into_iter().collect())
    }

    fn read_file_from_commit(&self, commit_oid: &str, rel_path: &str) -> Option<String> {
        self.repo.read_file_at(commit_oid, rel_path).ok()
    }

    /// Try to determine crdt_strategy for a set of conflict files by looking up the
    /// doogat type in the first conflict's content, then reading the typedef.
    fn lookup_crdt_strategy_for_conflicts(
        &self,
        conflicts: &[ConflictFile],
        index: &Index,
    ) -> Option<String> {
        let first = conflicts.first()?;
        let zones = parser::split_zones(&first.ours).ok()?;
        let meta = parser::parse_frontmatter(&zones.raw_frontmatter, &first.path).ok()?;
        let doogat_type = meta.doogat_type?;

        // Look up typedef path from index, then read and extract crdt_strategy
        let typedef_path = index.find_typedef_path(&doogat_type).ok()??;
        let content = self.repo.read_file(&typedef_path).ok()?;
        let typedef = parser::parse(&content, &typedef_path).ok()?;
        typedef
            .meta
            .extra
            .get("crdt_strategy")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// After the merge commit, reassign IDs for collision losers and rewrite links.
    fn reassign_collision_losers(
        &self,
        losers: Vec<CollisionLoser>,
        theirs_oid: &CommitHash,
    ) -> Result<usize> {
        let mut count = 0;
        for loser in &losers {
            self.reassign_single_loser(loser, theirs_oid)?;
            count += 1;
        }
        Ok(count)
    }

    /// Generate a new ID for one collision loser, rewrite links, and commit.
    fn reassign_single_loser(&self, loser: &CollisionLoser, theirs_oid: &CommitHash) -> Result<()> {
        let winner_id = loser.old_id.clone();
        let loser_type = loser.type_name.as_deref();
        let loser_folder = loser.folder;
        let new_id = parser::generate_unique_id(|candidate| {
            self.id_exists_in_repo(candidate, &winner_id, loser_type, loser_folder)
        });

        let updated_content = update_frontmatter_id(&loser.content, &new_id.0)?;
        let new_path =
            crate::git_ops::doogat_path(&new_id.0, loser.type_name.as_deref(), loser.folder);

        let old_path_no_ext = loser.old_path.trim_end_matches(".md");
        let new_path_no_ext = new_path.trim_end_matches(".md");
        let rewrites = self.scan_and_rewrite_links(
            &loser.old_id,
            old_path_no_ext,
            &new_id.0,
            new_path_no_ext,
            &theirs_oid.0,
        )?;

        let mut files: Vec<(String, String)> = vec![(new_path.clone(), updated_content)];
        files.extend(rewrites);

        let file_refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_files(
            &file_refs,
            &format!(
                "fix: reassign collided doogat ID {} -> {}",
                loser.old_id, new_id.0
            ),
        )?;

        tracing::warn!(
            old_id = %loser.old_id,
            new_id = %new_id.0,
            old_path = %loser.old_path,
            new_path = %new_path,
            "collision resolved: doogat ID reassigned"
        );
        Ok(())
    }

    /// Check whether a candidate ID already exists in the repo (winner ID or on-disk).
    fn id_exists_in_repo(
        &self,
        candidate: &str,
        winner_id: &str,
        loser_type: Option<&str>,
        loser_folder: bool,
    ) -> bool {
        if candidate == winner_id {
            return true;
        }
        let flat = crate::git_ops::doogat_path(candidate, None, false);
        if self.repo.read_file(&flat).is_ok() {
            return true;
        }
        if loser_folder {
            let typed = crate::git_ops::doogat_path(candidate, loser_type, true);
            return self.repo.read_file(&typed).is_ok();
        }
        false
    }

    /// Walk the HEAD tree and rewrite wikilinks from old ID/path to new ID/path.
    /// Skips files where theirs' (winner's) tree version already references the
    /// old ID - those links point to the winner and should remain unchanged.
    fn scan_and_rewrite_links(
        &self,
        old_id: &str,
        old_path_no_ext: &str,
        new_id: &str,
        new_path_no_ext: &str,
        theirs_oid: &str,
    ) -> Result<Vec<(String, String)>> {
        let head_oid = self.repo.head_oid()?;
        let head_files = self.repo.walk_tree_files(&head_oid.0, "ddb/")?;
        let mut rewrites = Vec::new();

        for (full_path, content) in &head_files {
            if !full_path.ends_with(".md") {
                continue;
            }
            if content.contains(old_id) {
                // Skip if theirs' version of this file also references the
                // old ID - that reference is to the winner, not the loser.
                if let Ok(theirs_content) = self.repo.read_file_at(theirs_oid, full_path) {
                    if theirs_content.contains(old_id) {
                        continue;
                    }
                }

                let rewritten = parser::rewrite_links(content, old_id, new_id);
                let rewritten = parser::rewrite_links(&rewritten, old_path_no_ext, new_path_no_ext);
                if rewritten != *content {
                    rewrites.push((full_path.clone(), rewritten));
                }
            }
        }

        Ok(rewrites)
    }

    /// Detect and mark nodes as stale if they haven't synced within `stale_ttl_days`.
    pub fn detect_stale_nodes(&self, stale_ttl_days: u32) -> Result<Vec<String>> {
        let nodes = self.list_nodes()?;
        let now = chrono::Utc::now();
        let ttl = chrono::Duration::days(stale_ttl_days as i64);
        let mut stale_uuids = Vec::new();

        for node in &nodes {
            if node.status == crate::types::NodeStatus::Retired {
                continue;
            }
            if let Some(ref last_sync) = node.last_sync {
                match chrono::DateTime::parse_from_rfc3339(last_sync) {
                    Ok(ts) => {
                        if now.signed_duration_since(ts) > ttl
                            && node.status != crate::types::NodeStatus::Stale
                        {
                            stale_uuids.push(node.uuid.clone());
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            "node {} has malformed last_sync '{}': {e}",
                            node.uuid,
                            last_sync
                        );
                    }
                }
            }
        }

        // Mark stale nodes
        for uuid in &stale_uuids {
            self.set_node_status(uuid, crate::types::NodeStatus::Stale)?;
        }

        Ok(stale_uuids)
    }

    /// Retire a node permanently.
    pub fn retire_node(&self, uuid: &str) -> Result<()> {
        self.set_node_status(uuid, crate::types::NodeStatus::Retired)
    }

    /// Reactivate a stale node (e.g. when it syncs again).
    pub fn reactivate_node(&self, uuid: &str) -> Result<()> {
        self.set_node_status(uuid, crate::types::NodeStatus::Active)
    }

    fn set_node_status(&self, uuid: &str, status: crate::types::NodeStatus) -> Result<()> {
        let node_path = format!(".nodes/{uuid}.toml");
        let toml_content = self.repo.read_file(&node_path)?;
        let mut node: NodeConfig = toml::from_str(&toml_content)?;
        node.status = status;
        let updated =
            toml::to_string_pretty(&node).map_err(|e| DoogatError::Parse(e.to_string()))?;
        self.repo
            .commit_file(&node_path, &updated, &format!("update node {uuid} status"))?;
        Ok(())
    }

    /// Tick the HLC for a local event and return the new timestamp.
    pub fn tick_hlc(&mut self) -> Hlc {
        let last = self.node.hlc.as_ref().and_then(|s| Hlc::parse(s).ok());
        let hlc = Hlc::now(&self.node.uuid, &last);
        self.node.hlc = Some(hlc.to_string());
        hlc
    }

    /// Merge a remote HLC into local state.
    pub fn recv_hlc(&mut self, remote: &Hlc) -> Hlc {
        let last = self.node.hlc.as_ref().and_then(|s| Hlc::parse(s).ok());
        let hlc = Hlc::recv(&self.node.uuid, &last, remote);
        self.node.hlc = Some(hlc.to_string());
        hlc
    }

    /// Update node's known_heads and last_sync.
    pub fn update_sync_state(&mut self) -> Result<()> {
        let head = self.repo.head_oid()?.to_string();
        self.node.known_heads = vec![head];
        self.node.last_sync = Some(chrono::Utc::now().to_rfc3339());

        let toml_content =
            toml::to_string_pretty(&self.node).map_err(|e| DoogatError::Parse(e.to_string()))?;
        let node_path = format!(".nodes/{}.toml", self.node.uuid);
        self.repo
            .commit_file(&node_path, &toml_content, "update sync state")?;

        Ok(())
    }

    /// Get the local node's UUID.
    pub fn local_uuid(&self) -> Result<String> {
        Ok(self.node.uuid.clone())
    }

    /// Resolve any conflicts left after a merge (e.g. from bundle import).
    /// Returns the number of conflicts resolved.
    pub fn resolve_post_merge_conflicts(&self, index: &crate::indexer::Index) -> Result<usize> {
        let head = self.repo.head_oid()?;
        let merge_hash = crate::types::CommitHash(head.to_string());
        self.validate_clean_merge_or_fallback(merge_hash, index)
    }
}

#[cfg(test)]
mod tests;
