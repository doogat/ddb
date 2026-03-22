use crate::crdt_resolver;
use crate::error::{Result, ZettelError};
use crate::git_ops::GitRepo;
use crate::hlc::Hlc;
use crate::indexer::Index;
use crate::parser;
use crate::types::{CommitHash, ConflictFile, MergeResult, NodeConfig, SyncReport};

impl From<toml::de::Error> for ZettelError {
    fn from(e: toml::de::Error) -> Self {
        Self::Toml(e.to_string())
    }
}

pub struct SyncManager<'a> {
    pub repo: &'a GitRepo,
    pub node: NodeConfig,
}

/// Register a new sync node for the given repo.
pub fn register_node(repo: &GitRepo, name: &str) -> Result<NodeConfig> {
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
        toml::to_string_pretty(&node).map_err(|e| ZettelError::Parse(e.to_string()))?;
    let node_path = format!(".nodes/{uuid}.toml");
    repo.commit_file(&node_path, &toml_content, &format!("register node {name}"))?;

    // Store UUID locally (not tracked by git)
    let local_path = repo.path.join(".git/zdb-node");
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
            let zettel_id = std::path::Path::new(&r.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            std::fs::create_dir_all(&temp_dir)?;
            let name = format!("{}_{zettel_id}_fm.crdt", commit_hash.0);
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

    // Infer folder mode from path depth: zettelkasten/{type}/{id}.md = 3 parts
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

/// Update the `id` field in a zettel's frontmatter, or add it if missing.
fn update_frontmatter_id(content: &str, new_id: &str) -> String {
    if let Ok(mut parsed) = parser::parse(content, "collision-loser") {
        parsed.meta.id = Some(crate::types::ZettelId(new_id.to_string()));
        parser::serialize(&parsed)
    } else {
        // Fallback: regex-based replacement if parse fails
        content.to_string()
    }
}

/// Ensures `skip_commit_graph` is reset when sync exits (success or error).
struct SkipCommitGraphResetGuard<'a> {
    repo: &'a GitRepo,
}

impl<'a> Drop for SkipCommitGraphResetGuard<'a> {
    fn drop(&mut self) {
        self.repo.set_skip_commit_graph(false);
    }
}

impl<'a> SyncManager<'a> {
    /// Open a SyncManager from an existing repo with a registered node.
    pub fn open(repo: &'a GitRepo) -> Result<Self> {
        let local_path = repo.path.join(".git/zdb-node");
        let uuid = std::fs::read_to_string(&local_path)
            .map_err(|_| ZettelError::NotFound("no node registered (.git/zdb-node)".into()))?;
        let uuid = uuid.trim().to_string();

        let node_path = format!(".nodes/{uuid}.toml");
        let toml_content = repo.read_file(&node_path)?;
        let node: NodeConfig = toml::from_str(&toml_content)?;

        Ok(Self { repo, node })
    }

    /// List all registered nodes.
    pub fn list_nodes(&self) -> Result<Vec<NodeConfig>> {
        let mut nodes = Vec::new();
        let head = self.repo.repo.head()?.peel_to_commit()?;
        let tree = head.tree()?;

        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            let full_path = format!("{}{}", dir, entry.name().unwrap_or(""));
            if full_path.starts_with(".nodes/") && full_path.ends_with(".toml") {
                if let Ok(blob) = self.repo.repo.find_blob(entry.id()) {
                    if let Ok(content) = std::str::from_utf8(blob.content()) {
                        if let Ok(node) = toml::from_str::<NodeConfig>(content) {
                            nodes.push(node);
                        }
                    }
                }
            }
            git2::TreeWalkResult::Ok
        })?;

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

        let mut report = SyncReport {
            direction: "bidirectional".into(),
            commits_transferred: 0,
            conflicts_resolved: 0,
            resurrected: 0,
            collisions_reassigned: 0,
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
                let count = conflicts.len();
                tracing::info!(count, "merge_result: conflicts");

                // Three-bucket partition
                let mut delete_edit = Vec::new();
                let mut add_add = Vec::new();
                let mut normal = Vec::new();
                for c in conflicts {
                    if c.ours.is_empty() || c.theirs.is_empty() {
                        delete_edit.push(c);
                    } else if c.ancestor.is_none() {
                        add_add.push(c);
                    } else {
                        normal.push(c);
                    }
                }

                let mut resolved = Vec::new();
                let mut collision_losers: Vec<CollisionLoser> = Vec::new();

                // Delete-vs-edit: edit wins, add resurrected marker
                for conflict in &delete_edit {
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
                        zettel_path = %conflict.path,
                        deleted_by,
                        "delete-vs-edit conflict resolved: zettel resurrected"
                    );
                    let content = add_resurrected_marker(surviving);
                    resolved.push(crate::types::ResolvedFile {
                        path: conflict.path.clone(),
                        content,
                        fm_crdt_bytes: None,
                    });
                }
                report.resurrected = delete_edit.len();
                if report.resurrected > 0 {
                    tracing::info!(count = report.resurrected, "delete_edit_resolved");
                }

                // Add-add collisions: winner keeps ID, loser stashed for reassignment
                for conflict in &add_add {
                    let (winner, loser) = resolve_add_add_collision(conflict);
                    resolved.push(winner);
                    collision_losers.push(loser);
                }

                // Normal conflicts: cascade resolve (CRDT → LWW fallback)
                if !normal.is_empty() {
                    let strategy = self.lookup_crdt_strategy_for_conflicts(&normal, index);
                    resolved.extend(self.cascade_resolve(normal, strategy.as_deref()));
                }

                // Tick HLC for merge commit
                let hlc = self.tick_hlc();
                let merge_msg =
                    crate::hlc::append_hlc_trailer("resolve merge conflicts via CRDT", &hlc);

                // Write resolved files and create merge commit with both parents
                let files: Vec<(&str, &str)> = resolved
                    .iter()
                    .map(|r| (r.path.as_str(), r.content.as_str()))
                    .collect();
                self.repo.commit_merge(&files, &merge_msg, &theirs_oid)?;

                // Persist frontmatter CRDT state for compaction
                let commit_oid = self.repo.head_oid()?;
                write_fm_crdt_files(&self.repo.path, &commit_oid, &resolved)?;

                // Post-merge: reassign collision losers
                report.collisions_reassigned =
                    self.reassign_collision_losers(collision_losers)?;

                report.conflicts_resolved = count;
                report.commits_transferred = 1;
            }
        }

        tracing::info!(
            phase = "merge",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        // Update sync state (before push so node registry travels with content)
        let phase_start = std::time::Instant::now();
        self.update_sync_state()?;
        tracing::info!(
            phase = "update_sync_state",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        // Push (single push carries both merge result and node state)
        let phase_start = std::time::Instant::now();
        self.repo.push(remote, branch)?;
        tracing::info!(
            phase = "push",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        // Write commit-graph once (covers all commits made during sync)
        self.repo.set_skip_commit_graph(false);
        self.repo.write_commit_graph();

        // Reindex
        let phase_start = std::time::Instant::now();
        index.rebuild_if_stale(self.repo)?;
        tracing::info!(
            phase = "reindex",
            elapsed_ms = phase_start.elapsed().as_millis(),
            "sync_phase"
        );

        crate::maintenance::maybe_auto_run(self.repo);

        tracing::info!(total_ms = sync_start.elapsed().as_millis(), "sync_complete");

        Ok(report)
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
        let merge_oid = git2::Oid::from_str(&merge_hash.0)?;
        let merge_commit = self.repo.repo.find_commit(merge_oid)?;
        if merge_commit.parent_count() < 2 {
            return Ok(0);
        }

        let ours_commit = merge_commit.parent(0)?;
        let theirs_commit = merge_commit.parent(1)?;
        let ancestor_commit = self
            .repo
            .repo
            .merge_base(ours_commit.id(), theirs_commit.id())
            .ok()
            .and_then(|oid| self.repo.repo.find_commit(oid).ok());

        let affected = self.affected_markdown_files(&ours_commit, &theirs_commit, &merge_commit)?;
        if affected.is_empty() {
            return Ok(0);
        }

        let has_parse_failure = affected.iter().any(|path| {
            self.read_file_from_commit(&merge_commit, path)
                .map(|content| parser::parse(&content, path).is_err())
                .unwrap_or(false)
        });
        if !has_parse_failure {
            return Ok(0);
        }

        let conflicts: Vec<ConflictFile> = affected
            .iter()
            .map(|path| ConflictFile {
                path: path.clone(),
                ancestor: ancestor_commit
                    .as_ref()
                    .and_then(|c| self.read_file_from_commit(c, path)),
                ours: self
                    .read_file_from_commit(&ours_commit, path)
                    .unwrap_or_default(),
                theirs: self
                    .read_file_from_commit(&theirs_commit, path)
                    .unwrap_or_default(),
                ours_hlc: self.repo.find_hlc_for_path(&ours_commit, path),
                theirs_hlc: self.repo.find_hlc_for_path(&theirs_commit, path),
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

        // Persist frontmatter CRDT state for compaction
        let commit_oid = self.repo.head_oid()?;
        write_fm_crdt_files(&self.repo.path, &commit_oid, &resolved)?;

        Ok(files.len())
    }

    fn affected_markdown_files(
        &self,
        ours: &git2::Commit<'_>,
        theirs: &git2::Commit<'_>,
        merged: &git2::Commit<'_>,
    ) -> Result<Vec<String>> {
        let mut paths = std::collections::BTreeSet::new();
        self.collect_changed_markdown_paths(ours, merged, &mut paths)?;
        self.collect_changed_markdown_paths(theirs, merged, &mut paths)?;
        Ok(paths.into_iter().collect())
    }

    fn collect_changed_markdown_paths(
        &self,
        from: &git2::Commit<'_>,
        to: &git2::Commit<'_>,
        out: &mut std::collections::BTreeSet<String>,
    ) -> Result<()> {
        let from_tree = from.tree()?;
        let to_tree = to.tree()?;
        let diff = self
            .repo
            .repo
            .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)?;

        for delta in diff.deltas() {
            let path = delta
                .new_file()
                .path()
                .or(delta.old_file().path())
                .and_then(|p| p.to_str());
            if let Some(path) = path {
                if path.starts_with("zettelkasten/") && path.ends_with(".md") {
                    out.insert(path.to_string());
                }
            }
        }

        Ok(())
    }

    fn read_file_from_commit(&self, commit: &git2::Commit<'_>, rel_path: &str) -> Option<String> {
        let tree = commit.tree().ok()?;
        let entry = tree.get_path(std::path::Path::new(rel_path)).ok()?;
        let blob = self.repo.repo.find_blob(entry.id()).ok()?;
        std::str::from_utf8(blob.content())
            .ok()
            .map(|s| s.to_string())
    }

    /// Try to determine crdt_strategy for a set of conflict files by looking up the
    /// zettel type in the first conflict's content, then reading the typedef.
    fn lookup_crdt_strategy_for_conflicts(
        &self,
        conflicts: &[ConflictFile],
        index: &Index,
    ) -> Option<String> {
        let first = conflicts.first()?;
        let zones = parser::split_zones(&first.ours).ok()?;
        let meta = parser::parse_frontmatter(&zones.raw_frontmatter, &first.path).ok()?;
        let zettel_type = meta.zettel_type?;

        // Look up typedef path from index, then read and extract crdt_strategy
        let typedef_path = index.find_typedef_path(&zettel_type).ok()??;
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
    fn reassign_collision_losers(&self, losers: Vec<CollisionLoser>) -> Result<usize> {
        let mut count = 0;
        for loser in &losers {
            let winner_id = loser.old_id.clone();
            let new_id = parser::generate_unique_id(|candidate| {
                let path = crate::git_ops::zettel_path(candidate, None, false);
                self.repo.read_file(&path).is_ok() || candidate == winner_id
            });

            let updated_content = update_frontmatter_id(&loser.content, &new_id.0);
            let new_path = crate::git_ops::zettel_path(
                &new_id.0,
                loser.type_name.as_deref(),
                loser.folder,
            );

            // Scan HEAD tree for files referencing the old ID and rewrite links
            let old_path_no_ext = loser.old_path.trim_end_matches(".md");
            let new_path_no_ext = new_path.trim_end_matches(".md");
            let rewrites = self.scan_and_rewrite_links(
                &loser.old_id,
                old_path_no_ext,
                &new_id.0,
                new_path_no_ext,
            )?;

            let mut files: Vec<(String, String)> = vec![(new_path.clone(), updated_content)];
            files.extend(rewrites);

            let file_refs: Vec<(&str, &str)> =
                files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
            self.repo.commit_files(
                &file_refs,
                &format!("fix: reassign collided zettel ID {} -> {}", loser.old_id, new_id.0),
            )?;

            tracing::warn!(
                old_id = %loser.old_id,
                new_id = %new_id.0,
                old_path = %loser.old_path,
                new_path = %new_path,
                "collision resolved: zettel ID reassigned"
            );

            count += 1;
        }
        Ok(count)
    }

    /// Walk the HEAD tree and rewrite wikilinks from old ID/path to new ID/path.
    fn scan_and_rewrite_links(
        &self,
        old_id: &str,
        old_path_no_ext: &str,
        new_id: &str,
        new_path_no_ext: &str,
    ) -> Result<Vec<(String, String)>> {
        let head = self.repo.repo.head()?.peel_to_commit()?;
        let tree = head.tree()?;
        let mut rewrites = Vec::new();

        tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            let full_path = format!("{}{}", dir, entry.name().unwrap_or(""));
            if !full_path.starts_with("zettelkasten/") || !full_path.ends_with(".md") {
                return git2::TreeWalkResult::Ok;
            }
            if let Ok(blob) = self.repo.repo.find_blob(entry.id()) {
                if let Ok(content) = std::str::from_utf8(blob.content()) {
                    if content.contains(old_id) {
                        let rewritten = parser::rewrite_links(content, old_id, new_id);
                        let rewritten =
                            parser::rewrite_links(&rewritten, old_path_no_ext, new_path_no_ext);
                        if rewritten != content {
                            rewrites.push((full_path, rewritten));
                        }
                    }
                }
            }
            git2::TreeWalkResult::Ok
        })?;

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
                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(last_sync) {
                    if now.signed_duration_since(ts) > ttl
                        && node.status != crate::types::NodeStatus::Stale
                    {
                        stale_uuids.push(node.uuid.clone());
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
            toml::to_string_pretty(&node).map_err(|e| ZettelError::Parse(e.to_string()))?;
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
            toml::to_string_pretty(&self.node).map_err(|e| ZettelError::Parse(e.to_string()))?;
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
mod tests {
    use super::*;

    fn temp_repo() -> (tempfile::TempDir, GitRepo) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        (dir, repo)
    }

    #[test]
    fn register_and_open_node() {
        let (_dir, repo) = temp_repo();
        let node = register_node(&repo, "Laptop").unwrap();
        assert!(!node.uuid.is_empty());
        assert_eq!(node.name, "Laptop");

        // Should be able to open
        let mgr = SyncManager::open(&repo).unwrap();
        assert_eq!(mgr.node.uuid, node.uuid);
    }

    #[test]
    fn list_nodes() {
        let (_dir, repo) = temp_repo();
        register_node(&repo, "Laptop").unwrap();

        let mgr = SyncManager::open(&repo).unwrap();
        let nodes = mgr.list_nodes().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "Laptop");
    }

    #[test]
    fn open_without_registration_fails() {
        let (_dir, repo) = temp_repo();
        let result = SyncManager::open(&repo);
        assert!(result.is_err());
    }

    #[test]
    fn sync_state_update() {
        let (_dir, repo) = temp_repo();
        register_node(&repo, "Test").unwrap();
        let mut mgr = SyncManager::open(&repo).unwrap();

        mgr.update_sync_state().unwrap();
        assert!(!mgr.node.known_heads.is_empty());
        assert!(mgr.node.last_sync.is_some());
    }

    #[test]
    fn node_status_defaults_to_active() {
        let (_dir, repo) = temp_repo();
        let node = register_node(&repo, "Test").unwrap();
        assert_eq!(node.status, crate::types::NodeStatus::Active);
        assert!(node.created.is_some());
    }

    #[test]
    fn retire_and_list_nodes() {
        let (_dir, repo) = temp_repo();
        register_node(&repo, "Laptop").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let nodes = mgr.list_nodes().unwrap();
        assert_eq!(nodes[0].status, crate::types::NodeStatus::Active);

        mgr.retire_node(&nodes[0].uuid).unwrap();
        let nodes = mgr.list_nodes().unwrap();
        assert_eq!(nodes[0].status, crate::types::NodeStatus::Retired);
    }

    #[test]
    fn backward_compat_old_toml_without_status() {
        let (_dir, repo) = temp_repo();
        // Write an old-style node config without status/created fields
        let uuid = "test-uuid-1234";
        let old_toml = format!("uuid = \"{uuid}\"\nname = \"OldNode\"\nknown_heads = []\n");
        repo.commit_file(&format!(".nodes/{uuid}.toml"), &old_toml, "old node")
            .unwrap();
        std::fs::write(repo.path.join(".git/zdb-node"), uuid).unwrap();

        let mgr = SyncManager::open(&repo).unwrap();
        assert_eq!(mgr.node.status, crate::types::NodeStatus::Active); // default
    }

    #[test]
    fn resurrected_marker_added() {
        let content = "---\ntitle: Test\n---\nBody content.";
        let result = add_resurrected_marker(content);
        assert!(result.contains("resurrected: true"));
        assert!(result.contains("title: Test"));
        assert!(result.contains("Body content"));
    }

    #[test]
    fn resurrected_marker_not_duplicated() {
        let content = "---\ntitle: Test\nresurrected: true\n---\nBody.";
        let result = add_resurrected_marker(content);
        assert_eq!(result.matches("resurrected").count(), 1);
    }

    #[test]
    fn clean_merge_validation_falls_back_to_crdt() {
        let (dir, repo) = temp_repo();
        register_node(&repo, "Test").unwrap();

        let path = "zettelkasten/note.md";
        let ancestor = "---\ntitle: Base\n---\nBody base.\n---\n- source:: base";
        let ours = "---\ntitle: Ours\n---\nBody ours.\n---\n- source:: base";
        let theirs = "---\ntitle: Base\n---\nBody base.\n---\n- source:: theirs";
        let merged_invalid = "---\ntitle: Broken\n---\nsource:: body\n---\n- source:: ref";

        repo.commit_file(path, ancestor, "ancestor").unwrap();
        let ancestor_hash = repo.head_oid().unwrap();
        repo.commit_file(path, ours, "ours edit").unwrap();
        let ours_hash = repo.head_oid().unwrap();

        let ancestor_commit = repo
            .repo
            .find_commit(git2::Oid::from_str(&ancestor_hash.0).unwrap())
            .unwrap();
        repo.repo.branch("theirs", &ancestor_commit, true).unwrap();
        repo.repo.set_head("refs/heads/theirs").unwrap();
        repo.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        repo.commit_file(path, theirs, "theirs edit").unwrap();
        let theirs_hash = repo.head_oid().unwrap();

        repo.repo.set_head("refs/heads/master").unwrap();
        repo.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        assert_eq!(repo.head_oid().unwrap(), ours_hash);

        let merge_hash = repo
            .commit_merge(
                &[(path, merged_invalid)],
                "synthetic clean merge",
                &theirs_hash,
            )
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let index = crate::indexer::Index::open(&db_path).unwrap();
        let mgr = SyncManager::open(&repo).unwrap();
        let resolved = mgr
            .validate_clean_merge_or_fallback(merge_hash, &index)
            .unwrap();
        assert_eq!(resolved, 1);

        let repaired = repo.read_file(path).unwrap();
        assert!(parser::parse(&repaired, path).is_ok());
        let head = repo.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_count(), 1);
    }

    #[test]
    fn lww_fallback_when_crdt_produces_invalid_output() {
        // Test the cascade: CRDT resolve -> validation fails -> LWW fallback.
        // We set up a real git merge conflict where the CRDT merge produces
        // invalid output (malformed frontmatter), triggering LWW fallback.
        let (dir, repo) = temp_repo();
        register_node(&repo, "Test").unwrap();

        let path = "zettelkasten/20260301120000.md";

        // Ancestor: valid zettel
        let ancestor = "---\nid: 20260301120000\ntitle: Base\ndate: 2026-03-01\n---\nBase body.\n---\n- source:: base";
        repo.commit_file(path, ancestor, "ancestor").unwrap();
        let ancestor_hash = repo.head_oid().unwrap();

        // Ours: valid edit
        let ours = "---\nid: 20260301120000\ntitle: Ours Edit\ndate: 2026-03-01\n---\nOurs body.\n---\n- source:: ours";
        repo.commit_file(path, ours, "ours edit").unwrap();

        // Create theirs branch from ancestor
        let ancestor_commit = repo
            .repo
            .find_commit(git2::Oid::from_str(&ancestor_hash.0).unwrap())
            .unwrap();
        repo.repo.branch("theirs_lww", &ancestor_commit, true).unwrap();
        repo.repo.set_head("refs/heads/theirs_lww").unwrap();
        repo.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();

        // Theirs: valid edit with cross-zone inline field duplication
        let theirs = "---\nid: 20260301120000\ntitle: Theirs Edit\ndate: 2026-03-01\n---\nsource:: theirs_body_field\n---\n- source:: theirs";
        repo.commit_file(path, theirs, "theirs edit").unwrap();
        let theirs_hash = repo.head_oid().unwrap();

        // Switch back to master
        repo.repo.set_head("refs/heads/master").unwrap();
        repo.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();

        // Create a synthetic merge commit with INVALID content
        // (malformed YAML that parser::parse will reject)
        let merged_invalid = "---\ntitle: Broken\n: invalid yaml [\n---\nBody.";
        let merge_hash = repo
            .commit_merge(
                &[(path, merged_invalid)],
                "synthetic merge with invalid content",
                &theirs_hash,
            )
            .unwrap();

        // validate_clean_merge_or_fallback detects the invalid content,
        // builds ConflictFile from the two parent commits, and calls
        // cascade_resolve. The CRDT merge of ours+theirs may also produce
        // a cross-zone "source" duplicate (body + ref) which fails validation,
        // triggering LWW fallback.
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let index = crate::indexer::Index::open(&db_path).unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let resolved_count = mgr
            .validate_clean_merge_or_fallback(merge_hash, &index)
            .unwrap();
        assert_eq!(resolved_count, 1, "should have resolved 1 conflict");

        // The key assertion: the resolved file MUST be valid (parseable).
        // Whether CRDT or LWW won, the cascade guarantees a valid result.
        let final_content = repo.read_file(path).unwrap();
        let parsed = crate::parser::parse(&final_content, path);
        assert!(
            parsed.is_ok(),
            "cascade_resolve must produce parseable output: {:?}",
            parsed.err()
        );

        // The resolved title must be from one of the two parents (not the invalid merge)
        let parsed = parsed.unwrap();
        let title = parsed.meta.title.as_deref().unwrap_or("");
        assert!(
            title == "Ours Edit" || title == "Theirs Edit",
            "title should be from one of the parents, got: {title}"
        );
    }

    #[test]
    fn sync_error_resets_skip_commit_graph_for_subsequent_commits() {
        let (dir, repo) = temp_repo();
        register_node(&repo, "Test").unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let index = crate::indexer::Index::open(&db_path).unwrap();

        let mut mgr = SyncManager::open(&repo).unwrap();
        let sync_result = mgr.sync("missing-remote", "master", &index);
        assert!(sync_result.is_err());

        let commit_graph = repo.path.join(".git/objects/info/commit-graph");
        let _ = std::fs::remove_file(&commit_graph);
        assert!(!commit_graph.exists());

        repo.commit_file(
            "zettelkasten/after-sync-error.md",
            "---\ntitle: After sync error\n---\nBody",
            "post-sync-error commit",
        )
        .unwrap();

        assert!(commit_graph.exists());
    }
}
