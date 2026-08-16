//! Bundle export/import for air-gapped sync.
//!
//! Bundle format (tar):
//! ```text
//! bundle.tar
//! ├── manifest.toml
//! ├── objects.bundle    (git bundle)
//! ├── nodes/            (.toml files for node registrations)
//! │   └── {uuid}.toml
//! └── checksum.sha256
//! ```

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{DoogatError, Result};
use crate::sync_manager::SyncManager;
use crate::traits::GitBackend;
use crate::types::{BundleManifest, SyncReport};

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| DoogatError::InvalidPath(path.display().to_string()))
}

/// Export a delta bundle targeting a specific node.
/// Includes only commits the target hasn't seen (based on known_heads).
pub fn export_bundle(
    repo: &impl GitBackend,
    sync_mgr: &SyncManager<impl GitBackend>,
    target_uuid: &str,
    output: &Path,
) -> Result<PathBuf> {
    let nodes = sync_mgr.list_nodes()?;
    let target = nodes
        .iter()
        .find(|n| n.uuid == target_uuid)
        .ok_or_else(|| DoogatError::NotFound(format!("node {target_uuid}")))?;

    // Determine basis for delta
    let basis_args: Vec<String> = target.known_heads.iter().map(|h| format!("^{h}")).collect();

    let local_uuid = sync_mgr.local_uuid()?;
    let manifest = BundleManifest {
        source_node: local_uuid,
        target_node: target_uuid.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        format_version: 1,
    };

    build_tar_bundle(repo, &manifest, &basis_args, output)
}

/// Export a full bundle (all refs) for bootstrapping a new node.
pub fn export_full_bundle(
    repo: &impl GitBackend,
    sync_mgr: &SyncManager<impl GitBackend>,
    output: &Path,
) -> Result<PathBuf> {
    let local_uuid = sync_mgr.local_uuid()?;
    let manifest = BundleManifest {
        source_node: local_uuid,
        target_node: "*".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        format_version: 1,
    };

    build_tar_bundle(repo, &manifest, &[], output)
}

/// Import a bundle into the repository, triggering the merge protocol.
/// Unbundle git objects and fetch refs from the extracted bundle.
fn unbundle_git_objects(repo: &impl GitBackend, work_dir: &TempDir) -> Result<()> {
    let git_bundle_path = work_dir.path().join("objects.bundle");
    if !git_bundle_path.exists() {
        return Ok(());
    }

    let output = std::process::Command::new("git")
        .args(["bundle", "unbundle", path_to_str(&git_bundle_path)?])
        .current_dir(repo.repo_path())
        .output()?;
    if !output.status.success() {
        return Err(DoogatError::Sync(format!(
            "git bundle unbundle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let output = std::process::Command::new("git")
        .args([
            "fetch",
            "--no-prune",
            path_to_str(&git_bundle_path)?,
            "refs/heads/*:refs/remotes/bundle/*",
        ])
        .current_dir(repo.repo_path())
        .output()?;
    if !output.status.success() {
        return Err(DoogatError::Sync(format!(
            "git fetch from bundle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

/// Delete every ref under `refs/remotes/bundle/*`, not just `master`. The
/// fetch in `unbundle_git_objects` maps `refs/heads/*` onto
/// `refs/remotes/bundle/*`, so a multi-branch bundle creates several bundle
/// refs; a successful import must clear the whole namespace.
fn delete_all_bundle_refs(repo: &impl GitBackend) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["for-each-ref", "--format=%(refname)", "refs/remotes/bundle/"])
        .current_dir(repo.repo_path())
        .output()?;
    if !output.status.success() {
        return Err(DoogatError::Sync(format!(
            "git for-each-ref failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(branch) = line.strip_prefix("refs/remotes/bundle/") {
            repo.delete_remote_ref("bundle", branch)?;
        }
    }

    Ok(())
}

/// Merge bundle/master into local master, resolving conflicts via the same
/// libgit2 + CRDT pipeline the network-sync path uses (`SyncManager::apply_merge_result`).
/// Bundle import opts out of the unrelated-histories guard explicitly, because a
/// fresh repo importing an established bundle has no common ancestor by design.
///
/// No CLI `git merge`, no stderr parsing, no MERGE_HEAD. On the CONFLICTED path
/// `merge_commits` computes the merge entirely in memory, so a resolution failure
/// there leaves the repo exactly as it was before this call (see Risks — no
/// `git merge --abort` step is needed). That guarantee does NOT extend to the
/// clean-merge path: `perform_normal_merge` creates the merge commit and force-checks-out
/// the worktree before `apply_merge_result` validates it, so a failure after that
/// point leaves `HEAD` moved. No data is lost either way — the bundle ref survives
/// until the import lands.
fn merge_bundle_and_resolve<G: GitBackend>(
    sync_mgr: &mut SyncManager<G>,
    index: &crate::indexer::Index,
) -> Result<SyncReport> {
    let merge_result = sync_mgr
        .repo
        .merge_remote_allowing_unrelated("bundle", "master")
        .map_err(bundle_merge_error)?;
    sync_mgr
        .apply_merge_result(merge_result, index)
        .map_err(bundle_merge_error)
}

/// Map a merge-engine failure onto the documented bundle-import error
/// contract: every failure of the merge sequence surfaces as `Sync` with the
/// `"bundle merge failed: "` prefix, so no raw variant escapes `import_bundle`
/// and the same failure cannot report two different classes depending on which
/// call it came from.
///
/// This deliberately wraps `DoogatError::Conflict` too. `Conflict` does not
/// mean "retryable" in the merge path — it is overloaded across a retryable
/// class (write-lock acquire timeout; the resolve→commit window guard) and a
/// terminal one (a collision loser whose id cannot be rewritten; a binary
/// conflict missing its blob OID). Either wrapped call can raise either class —
/// `merge_remote` takes the write lock itself, and `apply_merge_result` raises
/// both the window guard and the terminal collision failure — so they cannot be
/// told apart at this boundary, and the terminal case is the one bundle import
/// must report as `Sync`.
///
/// The cost is that a genuinely retryable write-lock timeout also reports as
/// `Sync` (public category `Internal`) rather than `Conflict` (`409`). Fixing
/// that properly means reclassifying the terminal errors at their source, which
/// changes a public error shape and so belongs to its own PRD.
fn bundle_merge_error(e: DoogatError) -> DoogatError {
    DoogatError::Sync(format!("bundle merge failed: {e}"))
}

/// Import node registration files from the bundle into the repo.
fn import_node_registrations(repo: &impl GitBackend, work_dir: &TempDir) -> Result<()> {
    let nodes_dir = work_dir.path().join("nodes");
    if !nodes_dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&nodes_dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let content = std::fs::read_to_string(entry.path())?;
        let dest = repo.repo_path().join(".nodes").join(entry.file_name());
        if !dest.exists() {
            std::fs::write(&dest, &content)?;
        }
    }
    Ok(())
}

pub fn import_bundle(
    repo: &impl GitBackend,
    sync_mgr: &mut SyncManager<impl GitBackend>,
    index: &crate::indexer::Index,
    bundle_path: &Path,
) -> Result<SyncReport> {
    let work_dir = make_temp_dir()?;

    let file = std::fs::File::open(bundle_path)?;
    let mut archive = tar::Archive::new(file);
    archive.unpack(work_dir.path())?;
    verify_extracted_checksum(work_dir.path())?;

    let manifest_str = std::fs::read_to_string(work_dir.path().join("manifest.toml"))?;
    let _manifest: BundleManifest =
        toml::from_str(&manifest_str).map_err(|e| DoogatError::Toml(e.to_string()))?;

    unbundle_git_objects(repo, &work_dir)?;
    let mut report = merge_bundle_and_resolve(sync_mgr, index)?;
    import_node_registrations(repo, &work_dir)?;

    delete_all_bundle_refs(repo)?;

    index.rebuild(repo)?;

    report.direction = "bundle-import".to_string();
    Ok(report)
}

/// Parse and verify a bundle without importing.
pub fn verify_bundle(bundle_path: &Path) -> Result<BundleManifest> {
    let work_dir = make_temp_dir()?;

    let file = std::fs::File::open(bundle_path)?;
    let mut archive = tar::Archive::new(file);
    archive.unpack(work_dir.path())?;

    verify_extracted_checksum(work_dir.path())?;

    let manifest_str = std::fs::read_to_string(work_dir.path().join("manifest.toml"))?;
    let manifest: BundleManifest =
        toml::from_str(&manifest_str).map_err(|e| DoogatError::Toml(e.to_string()))?;

    Ok(manifest)
}

// --- Internal helpers ---

/// Temp dir that cleans up on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn make_temp_dir() -> Result<TempDir> {
    let path = std::env::temp_dir().join(format!("ddb-bundle-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path)?;
    Ok(TempDir(path))
}

fn build_tar_bundle(
    repo: &impl GitBackend,
    manifest: &BundleManifest,
    basis_args: &[String],
    output: &Path,
) -> Result<PathBuf> {
    let work_dir = make_temp_dir()?;

    // Write manifest
    let manifest_toml =
        toml::to_string_pretty(manifest).map_err(|e| DoogatError::Toml(e.to_string()))?;
    std::fs::write(work_dir.path().join("manifest.toml"), &manifest_toml)?;

    // Create git bundle
    let bundle_path = work_dir.path().join("objects.bundle");
    let mut args = vec![
        "bundle".to_string(),
        "create".to_string(),
        path_to_str(&bundle_path)?.to_string(),
    ];
    if basis_args.is_empty() {
        args.push("--all".to_string());
    } else {
        args.extend(basis_args.iter().cloned());
        args.push("refs/heads/master".to_string());
    }
    let output_cmd = std::process::Command::new("git")
        .args(&args)
        .current_dir(repo.repo_path())
        .output()?;
    if !output_cmd.status.success() {
        return Err(DoogatError::Sync(format!(
            "git bundle create failed: {}",
            String::from_utf8_lossy(&output_cmd.stderr)
        )));
    }

    // Copy node files
    let nodes_src = repo.repo_path().join(".nodes");
    if nodes_src.exists() {
        let nodes_dst = work_dir.path().join("nodes");
        std::fs::create_dir_all(&nodes_dst)?;
        for entry in std::fs::read_dir(&nodes_src)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".toml") {
                std::fs::copy(entry.path(), nodes_dst.join(name))?;
            }
        }
    }

    // Compute checksum of all files
    let checksum = compute_bundle_checksum(work_dir.path())?;
    std::fs::write(work_dir.path().join("checksum.sha256"), &checksum)?;

    // Create tar archive
    let output_path = output.to_path_buf();
    let tar_file = std::fs::File::create(&output_path)?;
    let mut builder = tar::Builder::new(tar_file);

    for entry in std::fs::read_dir(work_dir.path())? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if entry.file_type()?.is_dir() {
            builder.append_dir_all(name_str.as_ref(), entry.path())?;
        } else {
            builder.append_path_with_name(entry.path(), name_str.as_ref())?;
        }
    }

    builder.finish()?;

    Ok(output_path)
}

fn compute_bundle_checksum(dir: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name != "checksum.sha256"
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        if entry.file_type()?.is_dir() {
            hash_dir_recursive(&mut hasher, &entry.path())?;
        } else {
            let mut f = std::fs::File::open(entry.path())?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            hasher.update(entry.file_name().to_string_lossy().as_bytes());
            hasher.update(&buf);
        }
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_dir_recursive(hasher: &mut Sha256, dir: &Path) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        if entry.file_type()?.is_dir() {
            hash_dir_recursive(hasher, &entry.path())?;
        } else {
            let mut f = std::fs::File::open(entry.path())?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            hasher.update(entry.file_name().to_string_lossy().as_bytes());
            hasher.update(&buf);
        }
    }
    Ok(())
}

fn verify_extracted_checksum(dir: &Path) -> Result<()> {
    let checksum_path = dir.join("checksum.sha256");
    if !checksum_path.exists() {
        return Err(DoogatError::Validation(
            "bundle missing checksum.sha256".into(),
        ));
    }
    let expected = std::fs::read_to_string(&checksum_path)?.trim().to_string();
    let actual = compute_bundle_checksum(dir)?;
    if expected != actual {
        return Err(DoogatError::Validation(format!(
            "bundle checksum mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
