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
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{DoogatError, Result};
use crate::git_ops::write_lock::{self, WriteLockGuard};
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

/// How long an import waits for another cooperating importer to release the
/// bundle-import lease. Bounds the wait only, not the whole import.
const BUNDLE_IMPORT_LEASE_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `git` in the repo and return stdout, failing loud on a non-zero exit.
fn run_git(repo: &impl GitBackend, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo.repo_path())
        .output()?;
    if !output.status.success() {
        return Err(DoogatError::Sync(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| DoogatError::Sync(format!("git {} output is not UTF-8: {e}", args.join(" "))))
}

/// Internal payload id: full lowercase-hex SHA-256 of the `objects.bundle`
/// bytes, streamed. Metadata-only tar changes (manifest, nodes) keep the id.
fn compute_payload_id(git_bundle_path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(git_bundle_path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Take the repo-scoped bundle-import lease in Git's common metadata dir.
/// Serializes cooperating importers only; it is NOT the Git write lock.
fn acquire_import_lease(repo: &impl GitBackend, timeout: Duration) -> Result<WriteLockGuard> {
    let common = run_git(repo, &["rev-parse", "--git-common-dir"])?;
    let common = Path::new(common.trim_end_matches(['\n', '\r']));
    let lock_dir = if common.is_absolute() {
        common.to_path_buf()
    } else {
        repo.repo_path().join(common)
    };
    write_lock::acquire(&lock_dir, "ddb-bundle-import.lock", timeout).map_err(|e| match e {
        DoogatError::Conflict(_) => DoogatError::Conflict(format!(
            "timed out after {}ms waiting for the bundle-import lease ({}); \
             another bundle import is in progress",
            timeout.as_millis(),
            lock_dir.join("ddb-bundle-import.lock").display()
        )),
        other => other,
    })
}

/// Parse `<oid> <refname>` lines strictly into `(refname, oid)` pairs;
/// malformed records fail loud.
fn parse_ref_lines(text: &str, what: &str) -> Result<Vec<(String, String)>> {
    text.lines()
        .map(|line| -> Result<(String, String)> {
            let (oid, name) = line
                .split_once(' ')
                .filter(|(oid, name)| {
                    matches!(oid.len(), 40 | 64)
                        && oid.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                        && !name.is_empty()
                        && !name.contains(char::is_whitespace)
                })
                .ok_or_else(|| DoogatError::Sync(format!("malformed {what} record: {line:?}")))?;
            Ok((name.to_string(), oid.to_string()))
        })
        .collect()
}

/// Branches (`refs/heads/*`) this Git bundle advertises, as sorted
/// `(branch, oid)`. Advertised `HEAD` and tags never select a branch.
fn advertised_heads(
    repo: &impl GitBackend,
    git_bundle_path: &Path,
) -> Result<Vec<(String, String)>> {
    let out = run_git(
        repo,
        &["bundle", "list-heads", path_to_str(git_bundle_path)?],
    )?;
    let mut heads = Vec::new();
    for (name, oid) in parse_ref_lines(&out, "bundle head")? {
        if let Some(branch) = name.strip_prefix("refs/heads/") {
            if branch.is_empty() {
                return Err(DoogatError::Sync(format!("malformed bundle head: {name}")));
            }
            heads.push((branch.to_string(), oid));
        }
    }
    heads.sort();
    Ok(heads)
}

/// Refs currently under `namespace` (ends in `/`), as sorted `(branch, oid)`.
/// The exact trailing-slash boundary keeps sibling prefixes out.
fn namespace_refs(repo: &impl GitBackend, namespace: &str) -> Result<Vec<(String, String)>> {
    let out = run_git(
        repo,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            namespace,
        ],
    )?;
    let mut refs = Vec::new();
    for (name, oid) in parse_ref_lines(&out, "bundle ref")? {
        let branch = name.strip_prefix(namespace).ok_or_else(|| {
            DoogatError::Sync(format!("ref {name} listed outside namespace {namespace}"))
        })?;
        refs.push((branch.to_string(), oid));
    }
    refs.sort();
    Ok(refs)
}

/// Fail loud unless `namespace` holds exactly the advertised heads.
fn ensure_namespace_matches(
    repo: &impl GitBackend,
    namespace: &str,
    heads: &[(String, String)],
) -> Result<()> {
    let refs = namespace_refs(repo, namespace)?;
    if refs != heads {
        return Err(DoogatError::Sync(format!(
            "bundle refs under {namespace} {refs:?} do not match the advertised heads {heads:?}; \
             refs are kept for operator recovery"
        )));
    }
    Ok(())
}

/// Unbundle git objects and fetch the advertised heads into `namespace`,
/// never pruning and never fetching tags. No `+`/`--force` is passed, but git
/// still accepts non-fast-forward updates under `refs/remotes/`, so callers
/// check every preexisting namespace ref against the advertised heads first.
fn unbundle_git_objects(
    repo: &impl GitBackend,
    git_bundle_path: &Path,
    namespace: &str,
) -> Result<()> {
    let bundle = path_to_str(git_bundle_path)?;
    run_git(repo, &["bundle", "unbundle", bundle])?;
    let refspec = format!("refs/heads/*:{namespace}*");
    run_git(
        repo,
        &["fetch", "--no-prune", "--no-tags", bundle, refspec.as_str()],
    )?;
    Ok(())
}

/// Validate `namespace` against this bundle's advertised heads, fetch into
/// it, re-check it and require a delivered `master`. Runs under the import
/// lease; returns the advertised heads.
fn fetch_into_namespace(
    repo: &impl GitBackend,
    git_bundle_path: &Path,
    payload_id: &str,
    namespace: &str,
) -> Result<Vec<(String, String)>> {
    let heads = advertised_heads(repo, git_bundle_path)?;
    let existing = namespace_refs(repo, namespace)?;
    if let Some((branch, oid)) = existing.iter().find(|r| !heads.contains(r)) {
        return Err(DoogatError::Sync(format!(
            "bundle import refused: unexpected ref {namespace}{branch} at {oid} is not an \
             advertised head of this bundle {heads:?}; nothing was fetched, merged or deleted"
        )));
    }

    unbundle_git_objects(repo, git_bundle_path, namespace)?;
    ensure_namespace_matches(repo, namespace, &heads)?;

    if !heads.iter().any(|(branch, _)| branch == "master") {
        let delivered: Vec<&str> = heads.iter().map(|(b, _)| b.as_str()).collect();
        return Err(DoogatError::Sync(format!(
            "bundle merge failed: bundle payload {payload_id} delivered no master branch \
             (delivered: {delivered:?}); fetched refs kept under {namespace}"
        )));
    }
    Ok(heads)
}

/// Delete exactly this payload's namespace refs after re-checking them.
/// Not transactional: an error here may leave the namespace partly deleted.
fn clean_namespace(
    repo: &impl GitBackend,
    payload_id: &str,
    namespace: &str,
    heads: &[(String, String)],
) -> Result<()> {
    ensure_namespace_matches(repo, namespace, heads)?;
    for (branch, _) in heads {
        repo.delete_remote_ref("bundle", &format!("{payload_id}/{branch}"))?;
    }
    Ok(())
}

/// Merge this payload's `refs/remotes/bundle/<payload-id>/master` into local
/// master, resolving conflicts via the same libgit2 + CRDT pipeline the
/// network-sync path uses (`SyncManager::apply_merge_result`).
/// Bundle import opts out of the unrelated-histories guard explicitly, because a
/// fresh repo importing an established bundle has no common ancestor by design.
///
/// No CLI `git merge`, no stderr parsing, no MERGE_HEAD. On the CONFLICTED path
/// `merge_commits` computes the merge entirely in memory, so a resolution failure
/// there leaves the repo exactly as it was before this call (see Risks — no
/// `git merge --abort` step is needed). That guarantee does NOT extend to the
/// clean-merge path: `perform_normal_merge` creates the merge commit and force-checks-out
/// the worktree before `apply_merge_result` validates it, so a failure after that
/// point leaves `HEAD` moved. No data is lost either way — the payload's
/// `refs/remotes/bundle/<payload-id>/` refs survive until the import lands.
fn merge_bundle_and_resolve<G: GitBackend>(
    sync_mgr: &mut SyncManager<G>,
    index: &crate::indexer::Index,
    payload_id: &str,
) -> Result<SyncReport> {
    let merge_failed = |e: DoogatError| {
        bundle_merge_error(format_args!(
            "{e} (fetched refs kept under refs/remotes/bundle/{payload_id}/)"
        ))
    };
    let merge_result = sync_mgr
        .repo
        .merge_remote_allowing_unrelated("bundle", &format!("{payload_id}/master"))
        .map_err(merge_failed)?;
    sync_mgr
        .apply_merge_result(merge_result, index)
        .map_err(merge_failed)
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
fn bundle_merge_error(e: impl std::fmt::Display) -> DoogatError {
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

/// Fetches the bundle's heads into `refs/remotes/bundle/<payload-id>/`,
/// merges its `master`, and deletes only that namespace once node
/// registration and the index rebuild succeed. Any earlier failure keeps
/// the fetched refs for a retry of the same payload.
pub fn import_bundle(
    repo: &impl GitBackend,
    sync_mgr: &mut SyncManager<impl GitBackend>,
    index: &crate::indexer::Index,
    bundle_path: &Path,
) -> Result<SyncReport> {
    import_bundle_with_lease_timeout(
        repo,
        sync_mgr,
        index,
        bundle_path,
        BUNDLE_IMPORT_LEASE_TIMEOUT,
    )
}

fn import_bundle_with_lease_timeout(
    repo: &impl GitBackend,
    sync_mgr: &mut SyncManager<impl GitBackend>,
    index: &crate::indexer::Index,
    bundle_path: &Path,
    lease_timeout: Duration,
) -> Result<SyncReport> {
    let work_dir = make_temp_dir()?;

    let file = std::fs::File::open(bundle_path)?;
    let mut archive = tar::Archive::new(file);
    archive.unpack(work_dir.path())?;
    verify_extracted_checksum(work_dir.path())?;

    let manifest_str = std::fs::read_to_string(work_dir.path().join("manifest.toml"))?;
    let _manifest: BundleManifest =
        toml::from_str(&manifest_str).map_err(|e| DoogatError::Toml(e.to_string()))?;

    let git_bundle_path = work_dir.path().join("objects.bundle");
    if !git_bundle_path.is_file() {
        return Err(DoogatError::Validation(
            "bundle missing objects.bundle".into(),
        ));
    }
    let payload_id = compute_payload_id(&git_bundle_path)?;
    let namespace = format!("refs/remotes/bundle/{payload_id}/");

    // Held from namespace inspection through final cleanup or error.
    let _lease = acquire_import_lease(repo, lease_timeout)?;

    let heads = fetch_into_namespace(repo, &git_bundle_path, &payload_id, &namespace)?;

    let mut report = merge_bundle_and_resolve(sync_mgr, index, &payload_id)?;
    if let Err(e) = import_node_registrations(repo, &work_dir).and_then(|()| index.rebuild(repo)) {
        tracing::warn!(%namespace, error = %e, "bundle import failed after merge; refs kept");
        return Err(e);
    }

    // Cleanup only after all post-merge work succeeded, and only this namespace.
    clean_namespace(repo, &payload_id, &namespace, &heads)?;

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
