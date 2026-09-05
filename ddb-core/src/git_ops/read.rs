use std::path::Path;

use git2::Oid;

use crate::error::{DoogatError, Result};
use crate::types::RepoConfig;

use super::{validate_path, GitRepo, CONFIG_FILE};

impl GitRepo {
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

    /// Walk ancestors of `commit` to find the HLC trailer of the commit that
    /// authored `path`'s current content. Each hop follows only the parent(s)
    /// whose entry for `path` equals the current commit's entry: a merge that
    /// kept one side's blob is transparent towards that side and opaque towards
    /// the discarded side, so a rejected lineage can never answer. A commit
    /// whose entry differs from every parent (or a root that has the path)
    /// authored it. Matching parents are followed breadth-first in parent
    /// order (PRD 00202 — diffing against `parent(0)` alone let a trailer-stamped
    /// merge outrank the real author, and a whole-DAG walk could answer from
    /// the discarded side).
    ///
    /// Returns `None` when no ancestor authored the path, when the authoring
    /// commit carries no HLC trailer (legacy history), or when the walk hits
    /// the depth cap.
    pub fn find_hlc_for_path(&self, commit: &git2::Commit, path: &str) -> Option<crate::hlc::Hlc> {
        const MAX_REVWALK_DEPTH: usize = 1000;
        let tree_path = Path::new(path);
        let entry_of = |tree: &git2::Tree<'_>| tree.get_path(tree_path).ok().map(|e| e.id());

        let mut queue = std::collections::VecDeque::from([commit.id()]);
        let mut seen = std::collections::HashSet::new();
        while let Some(oid) = queue.pop_front() {
            if !seen.insert(oid) {
                continue;
            }
            if seen.len() > MAX_REVWALK_DEPTH {
                tracing::warn!(path, depth = seen.len(), "HLC revwalk hit depth limit");
                return None;
            }
            let c = match self.repo.find_commit(oid) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(oid = %oid, error = %e, "skipping bad commit in HLC revwalk");
                    continue;
                }
            };
            let own = match c.tree() {
                Ok(tree) => entry_of(&tree),
                Err(e) => {
                    tracing::warn!(oid = %oid, error = %e, "skipping commit with bad tree in HLC revwalk");
                    queue.extend(c.parent_ids());
                    continue;
                }
            };
            let retained: Vec<git2::Oid> = c
                .parents()
                .filter(|parent| parent.tree().ok().as_ref().and_then(&entry_of) == own)
                .map(|parent| parent.id())
                .collect();
            let authored = if c.parent_count() == 0 {
                own.is_some()
            } else {
                retained.is_empty()
            };
            if authored {
                return crate::hlc::extract_hlc(c.message().unwrap_or(""));
            }
            queue.extend(retained);
        }
        None
    }

    /// Diff two commit OIDs, returning changed doogat paths with their change kind.
    pub fn diff_paths(
        &self,
        old_oid: &str,
        new_oid: &str,
    ) -> Result<Vec<(crate::types::DiffKind, String)>> {
        let (old_commit, new_commit) = self.resolve_commit_pair(old_oid, new_oid)?;

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
                        if let Some(kind) = Self::classify_delta(delta.status()) {
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

    fn resolve_commit_pair(
        &self,
        old_oid: &str,
        new_oid: &str,
    ) -> Result<(git2::Commit<'_>, git2::Commit<'_>)> {
        let old_commit = self
            .repo
            .find_commit(git2::Oid::from_str(old_oid).map_err(|e| DoogatError::Git(e.to_string()))?)
            .map_err(|e| DoogatError::Git(e.to_string()))?;
        let new_commit = self
            .repo
            .find_commit(git2::Oid::from_str(new_oid).map_err(|e| DoogatError::Git(e.to_string()))?)
            .map_err(|e| DoogatError::Git(e.to_string()))?;
        Ok((old_commit, new_commit))
    }

    fn classify_delta(status: git2::Delta) -> Option<crate::types::DiffKind> {
        use crate::types::DiffKind;
        match status {
            git2::Delta::Added => Some(DiffKind::Added),
            git2::Delta::Modified => Some(DiffKind::Modified),
            git2::Delta::Deleted => Some(DiffKind::Deleted),
            git2::Delta::Renamed => Some(DiffKind::Modified),
            _ => None,
        }
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
pub(super) fn format_git_time(commit: &git2::Commit) -> String {
    let time = commit.time();
    let offset = chrono::FixedOffset::east_opt(time.offset_minutes() * 60).unwrap_or_else(|| {
        chrono::FixedOffset::east_opt(0).expect("UTC offset zero is always valid")
    });
    chrono::DateTime::from_timestamp(time.seconds(), 0)
        .unwrap_or_default()
        .with_timezone(&offset)
        .to_rfc3339()
}
