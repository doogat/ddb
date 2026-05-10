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

/// Merge bundle/master into local master, resolving conflicts if needed.
/// Uses `--allow-unrelated-histories` because a freshly init'd repo may have
/// a different root commit than the bundle's history.
fn merge_bundle_and_resolve(
    repo: &impl GitBackend,
    sync_mgr: &mut SyncManager<impl GitBackend>,
    index: &crate::indexer::Index,
) -> Result<usize> {
    let merge_output = std::process::Command::new("git")
        .args([
            "merge",
            "refs/remotes/bundle/master",
            "--no-edit",
            "--allow-unrelated-histories",
        ])
        .current_dir(repo.repo_path())
        .output()?;

    if merge_output.status.success() {
        return Ok(0);
    }

    let stderr = String::from_utf8_lossy(&merge_output.stderr);
    if stderr.contains("CONFLICT") || stderr.contains("Automatic merge failed") {
        sync_mgr.resolve_post_merge_conflicts(index)
    } else {
        Err(DoogatError::Sync(format!("git merge failed: {stderr}")))
    }
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
    let conflicts_resolved = merge_bundle_and_resolve(repo, sync_mgr, index)?;
    import_node_registrations(repo, &work_dir)?;

    let _ = std::process::Command::new("git")
        .args(["update-ref", "-d", "refs/remotes/bundle/master"])
        .current_dir(repo.repo_path())
        .output();

    index.rebuild(repo)?;

    Ok(SyncReport {
        direction: "bundle-import".to_string(),
        commits_transferred: 0, // unbundle doesn't expose a count
        conflicts_resolved,
        resurrected: 0,
        collisions_reassigned: 0,
        singleton_conflicts_resolved: 0,
    })
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
mod tests {
    use super::*;
    use crate::git_ops::GitRepo;

    fn temp_repo() -> (::tempfile::TempDir, GitRepo) {
        let dir = ::tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.repo
            .config()
            .unwrap()
            .set_bool("commit.gpgsign", false)
            .unwrap();
        (dir, repo)
    }

    #[test]
    fn full_bundle_export_and_verify() {
        let (_dir, repo) = temp_repo();
        repo.commit_file(
            "ddb/20260301000000.md",
            "---\ntitle: test\n---\nBody",
            "add",
        )
        .unwrap();
        crate::sync_manager::register_node(&repo, "Node1").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let output = _dir.path().join("test.bundle.tar");
        let path = export_full_bundle(&repo, &mgr, &output).unwrap();
        assert!(path.exists());

        let manifest = verify_bundle(&path).unwrap();
        assert_eq!(manifest.target_node, "*");
        assert_eq!(manifest.format_version, 1);
    }

    #[test]
    fn checksum_verification_catches_tampering() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("ddb/20260301000000.md", "---\ntitle: test\n---\n", "add")
            .unwrap();
        crate::sync_manager::register_node(&repo, "Node1").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let output = _dir.path().join("test.bundle.tar");
        export_full_bundle(&repo, &mgr, &output).unwrap();

        // Tamper with the tar: extract, modify, repack
        let tamper_dir = _dir.path().join("tampered");
        std::fs::create_dir_all(&tamper_dir).unwrap();
        let file = std::fs::File::open(&output).unwrap();
        let mut archive = tar::Archive::new(file);
        archive.unpack(&tamper_dir).unwrap();

        // Modify manifest
        let manifest_path = tamper_dir.join("manifest.toml");
        let mut content = std::fs::read_to_string(&manifest_path).unwrap();
        content.push_str("\n# tampered\n");
        std::fs::write(&manifest_path, content).unwrap();

        // Repack
        let tampered_output = _dir.path().join("tampered.bundle.tar");
        let tar_file = std::fs::File::create(&tampered_output).unwrap();
        let mut builder = tar::Builder::new(tar_file);
        for entry in std::fs::read_dir(&tamper_dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if entry.file_type().unwrap().is_dir() {
                builder
                    .append_dir_all(name.to_string_lossy().as_ref(), entry.path())
                    .unwrap();
            } else {
                builder
                    .append_path_with_name(entry.path(), name.to_string_lossy().as_ref())
                    .unwrap();
            }
        }
        builder.finish().unwrap();

        let result = verify_bundle(&tampered_output);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("checksum mismatch"));
    }

    #[test]
    fn full_bundle_import_on_new_repo() {
        // Node 1: create content and export
        let (_dir1, repo1) = temp_repo();
        repo1
            .commit_file(
                "ddb/20260301000000.md",
                "---\ntitle: test\n---\nBody",
                "add",
            )
            .unwrap();
        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();

        let bundle_path = _dir1.path().join("full.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        // Node 2: import
        let (_dir2, repo2) = temp_repo();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();
        let db_path = _dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db_path).unwrap();

        let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path).unwrap();
        assert_eq!(report.direction, "bundle-import");

        // Verify content was imported
        let content = repo2.read_file("ddb/20260301000000.md").unwrap();
        assert!(content.contains("title: test"));
    }

    #[test]
    fn delta_export_targets_node_and_uses_known_heads() {
        let (_dir, repo) = temp_repo();

        // Create initial content
        repo.commit_file(
            "ddb/20260301000000.md",
            "---\ntitle: first\n---\nBody1",
            "add first",
        )
        .unwrap();
        crate::sync_manager::register_node(&repo, "Node1").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        // Record current head as node2's sync point
        let sync_point = repo.head_oid().unwrap().to_string();

        // Register a remote node with known_heads at sync_point
        let node2_uuid = "remote-node-2";
        let node2_config = format!(
            "uuid = \"{node2_uuid}\"\nname = \"Node2\"\nknown_heads = [\"{sync_point}\"]\n\
             status = \"Active\"\n"
        );
        repo.commit_file(
            &format!(".nodes/{node2_uuid}.toml"),
            &node2_config,
            "register node2",
        )
        .unwrap();

        // Add new content after node2's sync point
        repo.commit_file(
            "ddb/20260302000000.md",
            "---\ntitle: second\n---\nBody2",
            "add second",
        )
        .unwrap();

        // Export delta bundle targeting node2
        let output = _dir.path().join("delta.bundle.tar");
        let path = export_bundle(&repo, &mgr, node2_uuid, &output).unwrap();
        assert!(path.exists());

        // Verify manifest targets the specific node (not "*" like full export)
        let manifest = verify_bundle(&path).unwrap();
        assert_eq!(manifest.target_node, node2_uuid);
        assert_eq!(manifest.format_version, 1);

        // Verify the delta bundle is smaller than a full export
        let full_output = _dir.path().join("full.bundle.tar");
        export_full_bundle(&repo, &mgr, &full_output).unwrap();
        let delta_size = std::fs::metadata(&path).unwrap().len();
        let full_size = std::fs::metadata(&full_output).unwrap().len();
        assert!(
            delta_size < full_size,
            "delta ({delta_size}B) should be smaller than full ({full_size}B)"
        );
    }

    #[test]
    fn delta_export_fails_for_unknown_node() {
        let (_dir, repo) = temp_repo();
        repo.commit_file("ddb/20260301000000.md", "---\ntitle: test\n---\n", "add")
            .unwrap();
        crate::sync_manager::register_node(&repo, "Node1").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        let output = _dir.path().join("delta.bundle.tar");
        let result = export_bundle(&repo, &mgr, "nonexistent-uuid", &output);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nonexistent-uuid"));
    }
}
