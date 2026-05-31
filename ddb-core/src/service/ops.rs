use std::path::{Path, PathBuf};

use crate::error::{DoogatError, Result};
use crate::sync_manager::SyncManager;
use crate::traits::{GitBackend, IndexPort};
use crate::types::{
    CompactDryRunInfo, CompactOptions, CompactionReport, MaintenanceReport, NodeConfig,
};

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    // ── Sync / Compact / Maintenance ────────────────────────────────────

    pub fn compact(&self, opts: &CompactOptions) -> Result<CompactionReport> {
        let mgr = match SyncManager::open(&self.repo) {
            Ok(m) => m,
            Err(DoogatError::NotFound(_)) => {
                return Ok(CompactionReport {
                    files_removed: 0,
                    crdt_docs_compacted: 0,
                    gc_success: false,
                    crdt_temp_bytes_before: 0,
                    crdt_temp_bytes_after: 0,
                    crdt_temp_files_before: 0,
                    crdt_temp_files_after: 0,
                    repo_bytes_before: 0,
                    repo_bytes_after: 0,
                    backup_path: None,
                });
            }
            Err(e) => return Err(e),
        };
        crate::compaction::compact(&self.repo, &mgr, opts)
    }

    /// Dry-run compaction: return info without modifying anything.
    pub fn compact_dry_run(&self) -> Result<CompactDryRunInfo> {
        let nodes = self.list_nodes()?;
        let shared_head = crate::compaction::shared_head(&self.repo, &nodes)?;
        let temp_dir = self.repo_path.join(".crdt/temp");
        let crdt_temp_files = if temp_dir.exists() {
            std::fs::read_dir(&temp_dir)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .filter(|e| e.file_name().to_string_lossy() != ".gitkeep")
                        .count()
                })
                .unwrap_or(0)
        } else {
            0
        };
        let default_backup_path = crate::compaction::default_backup_path(&self.repo);
        Ok(CompactDryRunInfo {
            shared_head,
            crdt_temp_files,
            default_backup_path,
        })
    }

    /// Enable or disable auto-maintenance, persisted in .ddb.toml.
    pub fn set_auto_maintenance(&self, enabled: bool) -> Result<()> {
        let mut config = self.repo.load_config().unwrap_or_default();
        config.maintenance.auto_enabled = enabled;
        let toml_str =
            toml::to_string_pretty(&config).map_err(|e| DoogatError::Toml(e.to_string()))?;
        self.repo.commit_file(
            ".ddb.toml",
            &toml_str,
            &format!("maintenance auto {}", if enabled { "on" } else { "off" }),
        )?;
        Ok(())
    }

    /// Check if auto-maintenance is enabled.
    pub fn auto_maintenance_enabled(&self) -> Result<bool> {
        let config = self.load_config()?;
        Ok(config.maintenance.auto_enabled)
    }

    pub fn run_maintenance(&self, tasks: Option<&[&str]>) -> Result<MaintenanceReport> {
        crate::maintenance::run(self.repo.repo_path(), tasks)
    }

    pub fn register_node(&self, name: &str) -> Result<NodeConfig> {
        crate::sync_manager::register_node(&self.repo, name)
    }

    pub fn list_nodes(&self) -> Result<Vec<NodeConfig>> {
        let mgr = SyncManager::open(&self.repo)?;
        mgr.list_nodes()
    }

    pub fn retire_node(&self, uuid: &str) -> Result<()> {
        let mgr = SyncManager::open(&self.repo)?;
        mgr.retire_node(uuid)
    }

    // ── Bundles ─────────────────────────────────────────────────────────

    pub fn export_full_bundle(&self, output: &Path) -> Result<PathBuf> {
        let mgr = SyncManager::open(&self.repo)?;
        crate::bundle::export_full_bundle(&self.repo, &mgr, output)
    }

    pub fn export_delta_bundle(&self, target_uuid: &str, output: &Path) -> Result<PathBuf> {
        let mgr = SyncManager::open(&self.repo)?;
        crate::bundle::export_bundle(&self.repo, &mgr, target_uuid, output)
    }

}
