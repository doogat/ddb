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
fn merge_bundle_and_resolve(
    repo: &impl GitBackend,
    sync_mgr: &mut SyncManager<impl GitBackend>,
    index: &crate::indexer::Index,
) -> Result<SyncReport> {
    let merge_result = repo
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
    let mut report = merge_bundle_and_resolve(repo, sync_mgr, index)?;
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
mod tests {
    use super::*;
    use crate::git_ops::GitRepo;
    use crate::traits::GitHistory;

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
    fn conflicting_full_bundle_import_resolves_with_real_merge_commit() {
        // Node 1: create the shared ancestor commit.
        let (dir1, repo1) = temp_repo();
        let path = "ddb/20260301000000.md";
        repo1
            .commit_file(
                path,
                "---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n",
                "add ancestor",
            )
            .unwrap();

        // Node 2: clone Node 1's repo at this point so both nodes share the
        // ancestor commit as a real merge base.
        let dir2 = ::tempfile::TempDir::new().unwrap();
        git2::Repository::clone(dir1.path().to_str().unwrap(), dir2.path()).unwrap();
        let repo2 = GitRepo::open(dir2.path()).unwrap();
        repo2
            .repo
            .config()
            .unwrap()
            .set_bool("commit.gpgsign", false)
            .unwrap();

        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();

        // Both nodes edit the SAME line of the SAME doogat differently,
        // diverging from the shared ancestor commit above -- a real conflict.
        repo1
            .commit_file(
                path,
                "---\nid: 20260301000000\ntitle: Ancestor\n---\nNode1 body\n",
                "Node1 edits",
            )
            .unwrap();
        repo2
            .commit_file(
                path,
                "---\nid: 20260301000000\ntitle: Ancestor\n---\nNode2 body\n",
                "Node2 edits",
            )
            .unwrap();

        // Node1 exports a full bundle; Node2 imports it.
        let bundle_path = dir1.path().join("conflict.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path)
            .expect("a resolvable conflict must not fail the import");

        assert!(
            report.conflicts_resolved > 0,
            "a real conflict must be counted as resolved, not silently dropped (got {})",
            report.conflicts_resolved
        );

        let head = repo2.head_oid().unwrap().0;
        assert_eq!(
            repo2.commit_parent_count(&head).unwrap(),
            2,
            "a resolved conflict must land in a real 2-parent merge commit"
        );

        assert!(
            repo2
                .repo
                .find_reference("refs/remotes/bundle/master")
                .is_err(),
            "the bundle ref must be deleted after a successful import"
        );

        let content = repo2.read_file(path).unwrap();
        assert!(
            content.contains("id: 20260301000000"),
            "the resolved doogat must still be readable at HEAD, got: {content}"
        );
    }

    /// Reachability: a bundle-import merge failure (add/add collision loser
    /// with no frontmatter, so `rewrite_id_field` cannot rewrite its id) must
    /// leave `refs/remotes/bundle/master` in place. `import_bundle` only
    /// deletes that ref after a successful merge, so bundle data must stay
    /// reachable for a retry when the merge itself fails.
    #[test]
    fn conflicting_bundle_import_leaves_bundle_ref_reachable_on_merge_failure() {
        // Node 1: create the shared ancestor commit.
        let (dir1, repo1) = temp_repo();
        repo1
            .commit_file(
                "ddb/20260301000000.md",
                "---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n",
                "add ancestor",
            )
            .unwrap();

        // Node 2: clone Node 1's repo at this point so both nodes share the
        // ancestor commit as a real merge base.
        let dir2 = ::tempfile::TempDir::new().unwrap();
        git2::Repository::clone(dir1.path().to_str().unwrap(), dir2.path()).unwrap();
        let repo2 = GitRepo::open(dir2.path()).unwrap();
        repo2
            .repo
            .config()
            .unwrap()
            .set_bool("commit.gpgsign", false)
            .unwrap();

        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();

        // Both nodes independently add a NEW doogat at the SAME path, absent
        // from the shared ancestor -- a real add/add collision. The losing
        // side (Node1, "theirs" from repo2's merge-remote perspective) has no
        // frontmatter block at all, so `rewrite_id_field` cannot rewrite its
        // id and `commit_merge` must abort the whole merge commit atomically.
        let collision_path = "ddb/20260302000000.md";
        let loser_content = "Just a plain body with no frontmatter block at all.\n";
        assert!(
            crate::parser::rewrite_id_field(loser_content, "20260302999999").is_err(),
            "test setup invalid: rewrite_id_field must fail on frontmatter-less content"
        );

        // Node1 (theirs) is seeded with a strictly LOWER HLC so it loses the
        // add/add collision to Node2 (ours) under lww_pick's tie-break rule.
        let theirs_seed = crate::hlc::Hlc {
            wall_ms: u64::MAX / 2,
            counter: 0,
            node: "theirsss".into(),
        };
        std::fs::write(dir1.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
        repo1
            .commit_file(collision_path, loser_content, "Node1 adds loser")
            .unwrap();

        let ours_seed = crate::hlc::Hlc {
            wall_ms: u64::MAX / 2 + 1_000_000,
            counter: 0,
            node: "oursssss".into(),
        };
        std::fs::write(dir2.path().join(".git/ddb-hlc"), ours_seed.to_string()).unwrap();
        repo2
            .commit_file(
                collision_path,
                "---\nid: 20260302000000\ntitle: Winner\n---\nWinner body\n",
                "Node2 adds winner",
            )
            .unwrap();

        // Node1 exports a full bundle; Node2's import must fail because the
        // add/add collision loser cannot be rewritten.
        let bundle_path = dir1.path().join("collision.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
        assert!(
            matches!(result, Err(DoogatError::Sync(_))),
            "an unresolvable add/add collision must fail the import with a Sync error, got {result:?}"
        );

        assert!(
            repo2
                .repo
                .find_reference("refs/remotes/bundle/master")
                .is_ok(),
            "the bundle ref must survive a failed import so its data stays reachable"
        );
        assert!(
            repo2
                .repo
                .revparse_single("refs/remotes/bundle/master")
                .is_ok(),
            "the bundle ref must still resolve to the unbundled commits after a failed import"
        );
    }

    /// Clean-repo-on-error: the bundle-import merge path never shells out to
    /// CLI `git merge` (`merge_remote`/`merge_commits` compute the merge
    /// entirely in memory), so a merge failure must never leave a MERGE_HEAD
    /// file or unmerged index entries behind -- there is no `git merge
    /// --abort` step because none is needed.
    #[test]
    fn conflicting_bundle_import_leaves_repo_clean_on_merge_failure() {
        // Node 1: create the shared ancestor commit.
        let (dir1, repo1) = temp_repo();
        repo1
            .commit_file(
                "ddb/20260301000000.md",
                "---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n",
                "add ancestor",
            )
            .unwrap();

        // Node 2: clone Node 1's repo at this point so both nodes share the
        // ancestor commit as a real merge base.
        let dir2 = ::tempfile::TempDir::new().unwrap();
        git2::Repository::clone(dir1.path().to_str().unwrap(), dir2.path()).unwrap();
        let repo2 = GitRepo::open(dir2.path()).unwrap();
        repo2
            .repo
            .config()
            .unwrap()
            .set_bool("commit.gpgsign", false)
            .unwrap();

        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();

        // Both nodes independently add a NEW doogat at the SAME path, absent
        // from the shared ancestor -- a real add/add collision. The losing
        // side (Node1, "theirs" from repo2's merge-remote perspective) has no
        // frontmatter block at all, so `rewrite_id_field` cannot rewrite its
        // id and `commit_merge` must abort the whole merge commit atomically.
        let collision_path = "ddb/20260302000000.md";
        let loser_content = "Just a plain body with no frontmatter block at all.\n";
        assert!(
            crate::parser::rewrite_id_field(loser_content, "20260302999999").is_err(),
            "test setup invalid: rewrite_id_field must fail on frontmatter-less content"
        );

        let theirs_seed = crate::hlc::Hlc {
            wall_ms: u64::MAX / 2,
            counter: 0,
            node: "theirsss".into(),
        };
        std::fs::write(dir1.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
        repo1
            .commit_file(collision_path, loser_content, "Node1 adds loser")
            .unwrap();

        let ours_seed = crate::hlc::Hlc {
            wall_ms: u64::MAX / 2 + 1_000_000,
            counter: 0,
            node: "oursssss".into(),
        };
        std::fs::write(dir2.path().join(".git/ddb-hlc"), ours_seed.to_string()).unwrap();
        repo2
            .commit_file(
                collision_path,
                "---\nid: 20260302000000\ntitle: Winner\n---\nWinner body\n",
                "Node2 adds winner",
            )
            .unwrap();

        let bundle_path = dir1.path().join("collision.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
        assert!(
            matches!(result, Err(DoogatError::Sync(_))),
            "an unresolvable add/add collision must fail the import with a Sync error, got {result:?}"
        );

        assert!(
            !dir2.path().join(".git/MERGE_HEAD").exists(),
            "no CLI `git merge` runs on the bundle-import path, so no MERGE_HEAD file should ever exist"
        );
        assert!(
            !repo2.repo.index().unwrap().has_conflicts(),
            "an in-memory merge failure must never leave unmerged entries in the repo index"
        );
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

    /// Drive an import whose merge FAILS (add/add collision whose loser has
    /// no frontmatter), which by design leaves `refs/remotes/bundle/master`
    /// in place. Returns node 1's and node 2's temp dirs; node 2's repo is
    /// reopened by the caller so no borrow of it escapes this helper.
    fn stage_failed_bundle_import() -> (::tempfile::TempDir, ::tempfile::TempDir) {
        let (dir1, repo1) = temp_repo();
        repo1
            .commit_file(
                "ddb/20260301000000.md",
                "---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n",
                "add ancestor",
            )
            .unwrap();

        let dir2 = ::tempfile::TempDir::new().unwrap();
        git2::Repository::clone(dir1.path().to_str().unwrap(), dir2.path()).unwrap();
        let repo2 = GitRepo::open(dir2.path()).unwrap();
        repo2
            .repo
            .config()
            .unwrap()
            .set_bool("commit.gpgsign", false)
            .unwrap();

        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();

        let collision_path = "ddb/20260302000000.md";
        let theirs_seed = crate::hlc::Hlc {
            wall_ms: u64::MAX / 2,
            counter: 0,
            node: "theirsss".into(),
        };
        std::fs::write(dir1.path().join(".git/ddb-hlc"), theirs_seed.to_string()).unwrap();
        repo1
            .commit_file(
                collision_path,
                "Just a plain body with no frontmatter block at all.\n",
                "Node1 adds loser",
            )
            .unwrap();

        let ours_seed = crate::hlc::Hlc {
            wall_ms: u64::MAX / 2 + 1_000_000,
            counter: 0,
            node: "oursssss".into(),
        };
        std::fs::write(dir2.path().join(".git/ddb-hlc"), ours_seed.to_string()).unwrap();
        repo2
            .commit_file(
                collision_path,
                "---\nid: 20260302000000\ntitle: Winner\n---\nWinner body\n",
                "Node2 adds winner",
            )
            .unwrap();

        let bundle_path = dir1.path().join("collision.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
        assert!(
            matches!(result, Err(DoogatError::Sync(_))),
            "setup invalid: the collision import must fail, got {result:?}"
        );

        (dir1, dir2)
    }

    /// Export a full bundle whose branch set deliberately EXCLUDES `master`,
    /// so a pruning fetch into `refs/remotes/bundle/*` has something to prune.
    fn export_bundle_without_master(output: &Path) {
        let (_dir, repo) = temp_repo();
        repo.commit_file(
            "ddb/20260401000000.md",
            "---\nid: 20260401000000\ntitle: Sidecar\n---\nSidecar body\n",
            "add sidecar doogat",
        )
        .unwrap();
        crate::sync_manager::register_node(&repo, "Node3").unwrap();
        let mgr = SyncManager::open(&repo).unwrap();

        // Rehome the history onto a non-master branch, then drop `master`.
        let head = repo.repo.head().unwrap().peel_to_commit().unwrap();
        repo.repo.branch("sidecar", &head, false).unwrap();
        repo.repo.set_head("refs/heads/sidecar").unwrap();
        let mut master = repo
            .repo
            .find_branch("master", git2::BranchType::Local)
            .unwrap();
        master.delete().unwrap();

        export_full_bundle(&repo, &mgr, output).unwrap();
    }

    /// Reachability under `fetch.prune`: a failed import keeps
    /// `refs/remotes/bundle/master` so its data stays reachable for a retry.
    /// A LATER import of a different bundle must not silently take that
    /// reachability away, even when the repo has `fetch.prune = true` (a
    /// common global git setting) and the new bundle carries no `master`.
    #[test]
    fn kept_bundle_ref_survives_a_later_import_under_fetch_prune() {
        let (_dir1, dir2) = stage_failed_bundle_import();
        let repo2 = GitRepo::open(dir2.path()).unwrap();
        repo2
            .repo
            .config()
            .unwrap()
            .set_bool("fetch.prune", true)
            .unwrap();

        let kept = repo2
            .repo
            .revparse_single("refs/remotes/bundle/master")
            .expect("setup invalid: the failed import must have kept the bundle ref")
            .id();

        let other_bundle = dir2.path().join("other.bundle.tar");
        export_bundle_without_master(&other_bundle);

        let mut mgr2 = SyncManager::open(&repo2).unwrap();
        let index2 = crate::indexer::Index::open(&dir2.path().join(".ddb/index.db")).unwrap();
        // Whether this second import succeeds is not the property under test;
        // the first bundle's data staying reachable is.
        let _ = import_bundle(&repo2, &mut mgr2, &index2, &other_bundle);

        let survivor = repo2
            .repo
            .revparse_single("refs/remotes/bundle/master")
            .expect("a later import must not prune the bundle ref a failed import kept");
        assert_eq!(
            survivor.id(),
            kept,
            "the kept bundle ref must still name the first bundle's commit"
        );
        assert!(
            repo2.repo.find_commit(kept).is_ok(),
            "the commit the kept bundle ref names must still be present"
        );
    }

    /// Cleanup completeness: the import fetch maps `refs/heads/*` onto
    /// `refs/remotes/bundle/*`, so a multi-branch bundle creates several
    /// bundle refs. A successful import must clear the whole namespace, not
    /// just `master`, or stale bundle refs pile up in the repo.
    #[test]
    fn successful_import_deletes_every_bundle_ref_not_just_master() {
        let (dir1, repo1) = temp_repo();
        repo1
            .commit_file(
                "ddb/20260301000000.md",
                "---\nid: 20260301000000\ntitle: test\n---\nBody",
                "add",
            )
            .unwrap();
        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();

        // A second branch besides `master`, so the bundle carries two heads.
        let head1 = repo1.repo.head().unwrap().peel_to_commit().unwrap();
        repo1.repo.branch("feature", &head1, false).unwrap();

        let bundle_path = dir1.path().join("multi-branch.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        let (dir2, repo2) = temp_repo();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();
        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path)
            .expect("a clean multi-branch bundle must import successfully");
        assert_eq!(report.direction, "bundle-import");
        let content = repo2.read_file("ddb/20260301000000.md").unwrap();
        assert!(
            content.contains("title: test"),
            "the imported doogat must be readable at HEAD, got: {content}"
        );

        let mut refs = repo2.repo.references_glob("refs/remotes/bundle/*").unwrap();
        let leftover: Vec<String> = refs
            .names()
            .filter_map(|n| n.ok())
            .map(str::to_string)
            .collect();
        assert!(
            leftover.is_empty(),
            "a successful import must delete every bundle ref, not just master; leftover: {leftover:?}"
        );
    }

    /// A reassigned collision id must be a real `YYYYMMDDHHmmss` stamp, not
    /// merely fourteen digits: the reported regression minted
    /// `66177401297366` (month 74, hour 29, minute 73), which no repo-aware
    /// mint path could ever produce.
    fn is_calendar_shaped_id(id: &str) -> bool {
        let field = |from: usize, to: usize| id[from..to].parse::<u32>().unwrap_or(u32::MAX);
        crate::types::DoogatId::is_valid_shape(id)
            && (1..=12).contains(&field(4, 6))
            && (1..=31).contains(&field(6, 8))
            && field(8, 10) <= 23
            && field(10, 12) <= 59
            && field(12, 14) <= 59
    }

    /// Two separately `init`-ed repos have UNRELATED histories, so every
    /// conflicting path arrives with `ancestor: None`. A conflicting
    /// NON-doogat file (`.gitignore`) must be resolved by the ordinary
    /// conflict path; routing it into the doogat add/add collision resolver
    /// kills the documented full-bundle bootstrap with an unrecoverable
    /// frontmatter parse error.
    #[test]
    fn unrelated_history_import_resolves_conflicting_non_doogat_file() {
        let (dir1, repo1) = temp_repo();
        repo1
            .commit_file(".gitignore", "node_modules/\n", "Node1 gitignore")
            .unwrap();
        repo1
            .commit_file(
                "ddb/20260301000000.md",
                "---\nid: 20260301000000\ntitle: Node1 doc\n---\nNode1 body\n",
                "Node1 doogat",
            )
            .unwrap();
        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();

        let bundle_path = dir1.path().join("unrelated-nondoogat.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        // Node 2 is initialized on its own -- NOT cloned -- so the two
        // histories share no merge base.
        let (dir2, repo2) = temp_repo();
        repo2
            .commit_file(".gitignore", "target/\n", "Node2 gitignore")
            .unwrap();
        repo2
            .commit_file(
                "ddb/20260302000000.md",
                "---\nid: 20260302000000\ntitle: Node2 doc\n---\nNode2 body\n",
                "Node2 doogat",
            )
            .unwrap();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();
        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
        if let Err(err) = &result {
            let message = err.to_string();
            assert!(
                !message.contains("could not be rewritten: parse: no frontmatter opening ---"),
                "a conflicting non-doogat file must not be routed through the doogat add/add collision resolver, got: {message}"
            );
        }
        let report =
            result.expect("an unrelated-history import with a conflicting .gitignore must succeed");
        assert_eq!(report.direction, "bundle-import");
        assert_eq!(
            report.collisions_reassigned, 0,
            "no doogat id collides here, so nothing may be reassigned"
        );

        let gitignore = repo2.read_file(".gitignore").unwrap();
        assert!(
            gitignore.contains("target/") || gitignore.contains("node_modules/"),
            "the conflicting .gitignore must keep one side's content, got: {gitignore}"
        );

        assert!(repo2
            .read_file("ddb/20260301000000.md")
            .unwrap()
            .contains("Node1 body"));
        assert!(repo2
            .read_file("ddb/20260302000000.md")
            .unwrap()
            .contains("Node2 body"));
    }

    /// Same unrelated-history shape, but both sides independently hold the
    /// SAME doogat id. That IS a genuine add/add collision: both documents
    /// must survive and the reassigned loser must get a real 14-digit
    /// `YYYYMMDDHHmmss` id, never a silent duplicate under an invalid one.
    #[test]
    fn unrelated_history_import_reassigns_colliding_doogat_to_valid_id() {
        let path = "ddb/20260301000000.md";

        let (dir1, repo1) = temp_repo();
        repo1
            .commit_file(
                path,
                "---\nid: 20260301000000\ntitle: Node1\n---\nNode1 body\n",
                "Node1 adds",
            )
            .unwrap();
        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();

        let bundle_path = dir1.path().join("unrelated-collision.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        // Node 2 is initialized on its own -- NOT cloned -- so the two
        // histories share no merge base.
        let (dir2, repo2) = temp_repo();
        repo2
            .commit_file(
                path,
                "---\nid: 20260301000000\ntitle: Node2\n---\nNode2 body\n",
                "Node2 adds",
            )
            .unwrap();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();
        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let report = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path)
            .expect("a same-id collision across unrelated histories must resolve, not fail");
        assert_eq!(report.direction, "bundle-import");
        assert!(
            report.collisions_reassigned > 0,
            "the duplicated doogat id must be reported as a reassignment, got {}",
            report.collisions_reassigned
        );

        let tree = repo2
            .repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        let ddb_entry = tree
            .get_name("ddb")
            .expect("the ddb/ directory must exist at HEAD after an import");
        let ddb_tree = repo2.repo.find_tree(ddb_entry.id()).unwrap();
        let stems: Vec<String> = ddb_tree
            .iter()
            .filter_map(|entry| entry.name().ok().map(str::to_string))
            .filter_map(|name| name.strip_suffix(".md").map(str::to_string))
            .collect();

        assert_eq!(
            stems.len(),
            2,
            "both colliding documents must survive the reassignment, got {stems:?}"
        );
        for stem in &stems {
            assert!(
                is_calendar_shaped_id(stem),
                "every surviving doogat must live under a valid 14-digit YYYYMMDDHHmmss id, got ddb/{stem}.md"
            );
        }

        let bodies: Vec<String> = stems
            .iter()
            .map(|stem| repo2.read_file(&format!("ddb/{stem}.md")).unwrap())
            .collect();
        assert!(
            bodies.iter().any(|body| body.contains("Node1 body")),
            "the imported side must survive, got {bodies:?}"
        );
        assert!(
            bodies.iter().any(|body| body.contains("Node2 body")),
            "the local side must survive, got {bodies:?}"
        );
        for (stem, body) in stems.iter().zip(&bodies) {
            assert!(
                body.contains(&format!("id: {stem}")),
                "a reassigned doogat's frontmatter id must match its filename, got ddb/{stem}.md: {body}"
            );
        }
    }

    /// A `Conflict` from the merge sequence is wrapped as `Sync` like every
    /// other variant. `Conflict` is NOT a reliable "retryable" marker here: the
    /// merge path raises it both for genuinely retryable failures (write-lock
    /// acquire timeout, the resolve→commit window guard) and for terminal ones
    /// (a collision loser whose id cannot be rewritten). Either wrapped call can
    /// raise either class, and the terminal case is exactly what
    /// `conflicting_bundle_import_leaves_repo_clean_on_merge_failure` and
    /// `kept_bundle_ref_survives_a_later_import_under_fetch_prune` require to be
    /// reported as `Sync`. Letting `Conflict` through would break the documented
    /// "every bundle-import failure is a `Sync`" contract.
    #[test]
    fn bundle_merge_error_wraps_conflict_as_sync_because_conflict_is_not_retryable_here() {
        let message = match bundle_merge_error(DoogatError::Conflict(
            "collision loser at ddb/x.md could not be rewritten".to_string(),
        )) {
            DoogatError::Sync(msg) => msg,
            other => panic!("a merge-path Conflict must be wrapped as Sync, got {other:?}"),
        };
        assert!(
            message.starts_with("bundle merge failed: "),
            "message must start with the exact prefix, got: {message}"
        );
        assert!(
            message.contains("collision loser at ddb/x.md could not be rewritten"),
            "original error text must be preserved, got: {message}"
        );
    }

    #[test]
    fn bundle_merge_error_wraps_every_variant_as_sync_with_prefix() {
        let cases: Vec<(DoogatError, &str)> = vec![
            (
                DoogatError::NotFound("refs/remotes/bundle/master".to_string()),
                "refs/remotes/bundle/master",
            ),
            (
                DoogatError::Git("failed to fetch objects".to_string()),
                "failed to fetch objects",
            ),
            (
                DoogatError::Validation("bad frontmatter".to_string()),
                "bad frontmatter",
            ),
        ];

        for (input, original_text) in cases {
            let message = match bundle_merge_error(input) {
                DoogatError::Sync(msg) => msg,
                other => panic!("every merge-path error must be wrapped as Sync, got {other:?}"),
            };
            assert!(
                message.starts_with("bundle merge failed: "),
                "message must start with the exact prefix, got: {message}"
            );
            assert!(
                message.contains(original_text),
                "original error text must be preserved, got: {message}"
            );
        }
    }

    /// Reproduces the live failure: a bundle exported from a `main`-branch
    /// repo carries `refs/heads/main` but no `refs/heads/master`, so the
    /// merge engine's lookup of `refs/remotes/bundle/master` fails with
    /// `NotFound`. That raw variant must not escape `import_bundle` -- it
    /// must surface as the documented `Sync` contract.
    #[test]
    fn bundle_import_from_main_branch_repo_reports_sync_not_raw_not_found() {
        let (dir1, repo1) = temp_repo();
        repo1
            .commit_file(
                "ddb/20260301000000.md",
                "---\nid: 20260301000000\ntitle: test\n---\nBody\n",
                "add",
            )
            .unwrap();
        crate::sync_manager::register_node(&repo1, "Node1").unwrap();
        let mgr1 = SyncManager::open(&repo1).unwrap();

        // Rehome the history onto `main`, then drop `master` entirely.
        let head = repo1.repo.head().unwrap().peel_to_commit().unwrap();
        repo1.repo.branch("main", &head, false).unwrap();
        repo1.repo.set_head("refs/heads/main").unwrap();
        let mut master = repo1
            .repo
            .find_branch("master", git2::BranchType::Local)
            .unwrap();
        master.delete().unwrap();

        let bundle_path = dir1.path().join("main-branch.bundle.tar");
        export_full_bundle(&repo1, &mgr1, &bundle_path).unwrap();

        // A freshly-initialised repo merges `bundle/master`, which this
        // bundle never provides.
        let (dir2, repo2) = temp_repo();
        crate::sync_manager::register_node(&repo2, "Node2").unwrap();
        let mut mgr2 = SyncManager::open(&repo2).unwrap();
        let db2 = dir2.path().join(".ddb/index.db");
        std::fs::create_dir_all(db2.parent().unwrap()).unwrap();
        let index2 = crate::indexer::Index::open(&db2).unwrap();

        let result = import_bundle(&repo2, &mut mgr2, &index2, &bundle_path);
        assert!(
            !matches!(result, Err(DoogatError::NotFound(_))),
            "a merge-engine failure must not escape as a raw NotFound, got {result:?}"
        );
        let err = result.expect_err("importing a bundle with no master branch must fail");
        assert!(
            matches!(err, DoogatError::Sync(_)),
            "the merge-engine failure must surface as Sync, got {err:?}"
        );
        assert!(
            err.to_string().contains("bundle merge failed"),
            "the Sync message must contain the mapping prefix, got: {err}"
        );
    }
}
