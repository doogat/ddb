use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{DoogatError, Result};
use crate::sync_manager::SyncManager;
use crate::traits::GitBackend;
use crate::types::{CompactOptions, CompactionReport};

/// Compute the default backup path for a pre-compaction bundle.
pub fn default_backup_path(repo: &impl GitBackend) -> PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    repo.repo_path()
        .join(".ddb/backups")
        .join(format!("pre-compact-{ts}.bundle.tar"))
}

/// Create a pre-compaction backup bundle.
/// Default path: `.ddb/backups/pre-compact-{ISO8601}.bundle.tar`
pub fn backup_before_compact(
    repo: &impl GitBackend,
    sync_mgr: &SyncManager<impl GitBackend>,
    backup_path: Option<&Path>,
) -> Result<PathBuf> {
    let path = match backup_path {
        Some(p) => p.to_path_buf(),
        None => default_backup_path(repo),
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    crate::bundle::export_full_bundle(repo, sync_mgr, &path)
}

/// Find the greatest common ancestor commit reachable from all active nodes' known_heads.
/// Stale and retired nodes are excluded from the calculation.
pub fn shared_head(
    repo: &impl GitBackend,
    nodes: &[crate::types::NodeConfig],
) -> Result<Option<String>> {
    let heads: Vec<&String> = nodes
        .iter()
        .filter(|n| n.status == crate::types::NodeStatus::Active)
        .filter_map(|n| n.known_heads.first())
        .collect();

    if heads.is_empty() {
        return Ok(None);
    }
    if heads.len() == 1 {
        return Ok(Some(heads[0].clone()));
    }

    // Iteratively compute merge-base across all heads
    let mut base = repo.merge_base(heads[0], heads[1])?;
    for head in &heads[2..] {
        base = repo.merge_base(&base, head)?;
    }

    Ok(Some(base))
}

/// Parse doogat ID from CRDT temp filename.
/// Supports formats: `{oid}_{doogat_id}.crdt`, `{oid}_{doogat_id}_fm.crdt`,
/// and legacy `{oid}` or `{oid}.crdt`.
/// Returns `(oid_hex, doogat_id, is_frontmatter)`.
fn parse_crdt_temp_name(name: &str) -> Option<(String, Option<String>, bool)> {
    let stem = name.strip_suffix(".crdt").unwrap_or(name);

    if let Some((oid_part, rest)) = stem.split_once('_') {
        // Validate hex OID (40 hex chars)
        if oid_part.len() != 40 || !oid_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        if let Some(doogat_id) = rest.strip_suffix("_fm") {
            Some((oid_part.to_string(), Some(doogat_id.to_string()), true))
        } else {
            Some((oid_part.to_string(), Some(rest.to_string()), false))
        }
    } else {
        if stem.len() != 40 || !stem.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some((stem.to_string(), None, false))
    }
}

/// Remove temporary CRDT files older than the shared sync point.
pub fn cleanup_crdt_temp(repo: &impl GitBackend, shared_head: Option<&str>) -> Result<usize> {
    let temp_dir = repo.repo_path().join(".crdt/temp");
    if !temp_dir.exists() {
        return Ok(0);
    }

    let Some(shared_head) = shared_head else {
        return Ok(0);
    };

    let mut removed = 0;
    for entry in std::fs::read_dir(&temp_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".gitkeep" {
            continue;
        }
        let Some((temp_commit_oid, _doogat_id, _is_fm)) = parse_crdt_temp_name(&name) else {
            continue;
        };

        if repo
            .merge_base(shared_head, &temp_commit_oid)
            .ok()
            .as_deref()
            == Some(&temp_commit_oid)
        {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }

    Ok(removed)
}

/// Compact CRDT temp files by grouping per doogat and merging Automerge changes.
/// Returns the number of doogats whose CRDT docs were compacted.
pub fn compact_crdt_docs(repo: &impl GitBackend) -> Result<usize> {
    let temp_dir = repo.repo_path().join(".crdt/temp");
    if !temp_dir.exists() {
        return Ok(0);
    }

    // Group files by (doogat_id, is_frontmatter) so fm and body compact independently
    let mut by_key: HashMap<(String, bool), Vec<std::path::PathBuf>> = HashMap::new();
    for entry in std::fs::read_dir(&temp_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".gitkeep" {
            continue;
        }
        if let Some((_oid, Some(doogat_id), is_fm)) = parse_crdt_temp_name(&name) {
            by_key
                .entry((doogat_id, is_fm))
                .or_default()
                .push(entry.path());
        }
    }

    let mut compacted = 0;
    for ((doogat_id, is_fm), files) in &by_key {
        if files.len() < 2 {
            continue; // nothing to compact
        }

        // Load all Automerge changes and merge into a single doc
        let mut doc = automerge::AutoCommit::new();
        for file in files {
            if let Ok(data) = std::fs::read(file) {
                if let Ok(other) = automerge::AutoCommit::load(&data) {
                    doc.merge(&mut other.clone())
                        .map_err(|e| DoogatError::Automerge(e.to_string()))?;
                }
            }
        }

        // Save compacted doc with appropriate suffix
        let compacted_data = doc.save();
        let fm_suffix = if *is_fm { "_fm" } else { "" };
        let compacted_name = format!("compacted_{doogat_id}{fm_suffix}.crdt");
        std::fs::write(temp_dir.join(&compacted_name), compacted_data)?;

        // Remove original files
        for file in files {
            let _ = std::fs::remove_file(file);
        }

        compacted += 1;
    }

    Ok(compacted)
}

/// Compact CRDT docs for a single doogat.
pub fn compact_doogat(repo: &impl GitBackend, doogat_id: &str) -> Result<usize> {
    let temp_dir = repo.repo_path().join(".crdt/temp");
    if !temp_dir.exists() {
        return Ok(0);
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&temp_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some((_oid, Some(zid), _is_fm)) = parse_crdt_temp_name(&name) {
            if zid == doogat_id {
                files.push(entry.path());
            }
        }
    }

    if files.len() < 2 {
        return Ok(0);
    }

    let mut doc = automerge::AutoCommit::new();
    for file in &files {
        if let Ok(data) = std::fs::read(file) {
            if let Ok(other) = automerge::AutoCommit::load(&data) {
                doc.merge(&mut other.clone())
                    .map_err(|e| DoogatError::Automerge(e.to_string()))?;
            }
        }
    }

    let compacted_data = doc.save();
    let compacted_name = format!("compacted_{doogat_id}.crdt");
    std::fs::write(temp_dir.join(&compacted_name), compacted_data)?;

    for file in &files {
        let _ = std::fs::remove_file(file);
    }

    Ok(1)
}

/// Get total size and file count of `.crdt/temp/` directory.
fn crdt_temp_stats(repo: &impl GitBackend) -> (u64, usize) {
    let temp_dir = repo.repo_path().join(".crdt/temp");
    if !temp_dir.exists() {
        return (0, 0);
    }
    std::fs::read_dir(&temp_dir)
        .ok()
        .map(|entries| {
            let mut bytes = 0u64;
            let mut count = 0usize;
            for entry in entries.flatten() {
                if entry.file_name() == ".gitkeep" {
                    continue;
                }
                if let Ok(m) = entry.metadata() {
                    if m.is_file() {
                        bytes += m.len();
                        count += 1;
                    }
                }
            }
            (bytes, count)
        })
        .unwrap_or((0, 0))
}

/// Recursively compute total size of a directory in bytes.
fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(m) = entry.metadata() {
                if m.is_file() {
                    total += m.len();
                } else if m.is_dir() {
                    stack.push(entry.path());
                }
            }
        }
    }
    total
}

/// Run `git gc` on the repository.
pub fn run_gc(repo_path: &Path) -> Result<bool> {
    let output = std::process::Command::new("git")
        .args(["gc"])
        .current_dir(repo_path)
        .output()
        .map_err(DoogatError::Io)?;

    Ok(output.status.success())
}

/// Full compaction pipeline: threshold check → backup → shared head → cleanup → crdt doc compact → gc.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn compact(
    repo: &impl GitBackend,
    sync_mgr: &SyncManager<impl GitBackend>,
    opts: &CompactOptions,
) -> Result<CompactionReport> {
    // Threshold check: skip if under threshold (unless forced)
    if !opts.force {
        let config = repo.load_config()?;
        let (crdt_bytes, crdt_files) = crdt_temp_stats(repo);
        let size_mb = crdt_bytes as f64 / (1024.0 * 1024.0);
        if size_mb < config.compaction.threshold_mb as f64 {
            let repo_bytes = dir_size(&repo.repo_path().join(".git"));
            tracing::debug!(
                size_mb,
                threshold_mb = config.compaction.threshold_mb,
                "below_threshold_skip"
            );
            return Ok(CompactionReport {
                files_removed: 0,
                crdt_docs_compacted: 0,
                gc_success: true,
                crdt_temp_bytes_before: crdt_bytes,
                crdt_temp_bytes_after: crdt_bytes,
                crdt_temp_files_before: crdt_files,
                crdt_temp_files_after: crdt_files,
                repo_bytes_before: repo_bytes,
                repo_bytes_after: repo_bytes,
                backup_path: None,
            });
        }
    }

    // Pre-compaction backup
    let backup_path = if !opts.skip_backup {
        let bp = backup_before_compact(repo, sync_mgr, opts.backup_path.as_deref())?;
        tracing::info!(backup_path = %bp.display(), "pre_compaction_backup");
        Some(bp)
    } else {
        None
    };

    let (crdt_temp_bytes_before, crdt_temp_files_before) = crdt_temp_stats(repo);
    let repo_bytes_before = dir_size(&repo.repo_path().join(".git"));

    let nodes = sync_mgr.list_nodes()?;
    let head = shared_head(repo, &nodes)?;
    tracing::debug!(shared_head = ?head, node_count = nodes.len(), "shared_head_computed");
    let files_removed = cleanup_crdt_temp(repo, head.as_deref())?;
    if files_removed > 0 {
        tracing::info!(files_removed, "crdt_temp_cleanup");
    }

    let crdt_docs_compacted = compact_crdt_docs(repo)?;
    if crdt_docs_compacted > 0 {
        tracing::info!(crdt_docs_compacted, "crdt_docs_compacted");
    }

    let (crdt_temp_bytes_after, crdt_temp_files_after) = crdt_temp_stats(repo);

    let gc_success = run_gc(repo.repo_path())?;
    let repo_bytes_after = dir_size(&repo.repo_path().join(".git"));

    tracing::info!(
        gc_success,
        crdt_temp_bytes_before,
        crdt_temp_bytes_after,
        repo_bytes_before,
        repo_bytes_after,
        "compaction_result"
    );

    crate::maintenance::maybe_auto_run(repo);

    Ok(CompactionReport {
        files_removed,
        crdt_docs_compacted,
        gc_success,
        crdt_temp_bytes_before,
        crdt_temp_bytes_after,
        crdt_temp_files_before,
        crdt_temp_files_after,
        repo_bytes_before,
        repo_bytes_after,
        backup_path,
    })
}

#[cfg(test)]
mod tests;
