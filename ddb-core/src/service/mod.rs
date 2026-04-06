use std::path::{Path, PathBuf};

use crate::error::{DoogatError, Result};
use crate::git_ops::{self, GitRepo};
use crate::indexer::Index;
use crate::parser;
use crate::sql_engine::{SqlEngine, SqlResult, TransactionBuffer};
use crate::sync_manager::SyncManager;
use crate::traits::GitBackend;
use crate::types::{
    AttachmentInfo, BrokenSequence, CommitHash, CompactDryRunInfo, CompactOptions,
    CompactionReport, DoogatId, FixReport, LinkDensityEntry, MaintenanceReport, NodeConfig,
    OrphanDoogat, ParsedDoogat, RecentDoogat, RenameReport, SequenceInfo, SequenceNode,
    StaleDoogat, Suggestion, SyncReport, TableSchema, UnlinkedMention,
};

mod crud;
mod search;

/// Extra frontmatter fields to set or remove during an update.
pub struct ExtraFieldUpdates<'a> {
    pub set: &'a std::collections::BTreeMap<String, crate::types::Value>,
    pub unset: &'a [String],
}

impl Default for ExtraFieldUpdates<'_> {
    fn default() -> Self {
        static EMPTY_MAP: std::sync::LazyLock<
            std::collections::BTreeMap<String, crate::types::Value>,
        > = std::sync::LazyLock::new(std::collections::BTreeMap::new);
        Self {
            set: &EMPTY_MAP,
            unset: &[],
        }
    }
}

/// Unified orchestration layer composing GitRepo, Index, and optional NoSQL
/// index into a single entry point for all high-level operations.
///
/// CLI, FFI, and server consumers delegate to `DoogatService` instead of
/// independently composing core modules. This ensures consistent behaviour
/// (e.g. NoSQL dual-write) across all entry points.
pub struct DoogatService {
    pub(super) repo: GitRepo,
    pub(super) index: Index,
    pub(super) txn: Option<TransactionBuffer>,
    pub(super) repo_path: PathBuf,
    pub(super) skip_stale_check: bool,
}

impl DoogatService {
    /// Open an existing Doogat DB repository.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = GitRepo::open(path)?;
        let db_dir = path.join(".ddb");
        std::fs::create_dir_all(&db_dir)?;
        let index = Index::open(&db_dir.join("index.db"))?;
        Ok(Self {
            repo,
            index,
            txn: None,
            repo_path: path.to_path_buf(),
            skip_stale_check: false,
        })
    }

    /// Initialize a new Doogat DB repository at `path` and open it.
    pub fn init(path: &Path) -> Result<Self> {
        GitRepo::init(path)?;
        Self::open(path)
    }

    /// Path to the repository root.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Skip rebuild_if_stale checks. Use for read-only connections
    /// where another writer (e.g. actor) keeps the index fresh.
    pub fn set_skip_stale_check(&mut self, skip: bool) {
        self.skip_stale_check = skip;
    }

    fn ensure_fresh(&self) -> Result<()> {
        if !self.skip_stale_check {
            self.index.rebuild_if_stale(&self.repo)?;
        }
        // skip_stale_check: ReadPool path — actor keeps the index
        // current, so readers trust WAL visibility without querying
        // _ddb_meta (avoids SQLite lock contention on Windows).
        Ok(())
    }

    // ── SQL ─────────────────────────────────────────────────────────────

    pub fn execute_sql(&mut self, sql: &str) -> Result<SqlResult> {
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let result = engine.execute(sql).inspect_err(|_| {
            self.txn = engine.suspend_transaction();
        })?;
        self.txn = engine.suspend_transaction();
        Ok(result)
    }

    pub fn execute_batch(&mut self, sql: &str) -> Result<Vec<SqlResult>> {
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let results = engine.execute_batch(sql).inspect_err(|_| {
            self.txn = engine.suspend_transaction();
        })?;
        self.txn = engine.suspend_transaction();
        Ok(results)
    }

    pub fn begin_transaction(&mut self) -> Result<()> {
        if self.txn.is_some() {
            return Err(DoogatError::SqlEngine("transaction already active".into()));
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.execute("BEGIN")?;
        self.txn = engine.suspend_transaction();
        Ok(())
    }

    pub fn commit_transaction(&mut self) -> Result<()> {
        let buf = self
            .txn
            .take()
            .ok_or_else(|| DoogatError::SqlEngine("no active transaction".into()))?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        engine.execute("COMMIT").inspect_err(|_| {
            self.txn = engine.suspend_transaction();
        })?;
        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<()> {
        let buf = self
            .txn
            .take()
            .ok_or_else(|| DoogatError::SqlEngine("no active transaction".into()))?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        engine.execute("ROLLBACK")?;
        Ok(())
    }

    // ── Sync / Compact / Maintenance ────────────────────────────────────

    pub fn sync(&self, remote: &str, branch: &str) -> Result<SyncReport> {
        let mut mgr = match SyncManager::open(&self.repo) {
            Ok(m) => m,
            Err(DoogatError::NotFound(msg)) => {
                let node_file = self.repo.repo_path().join(".git/ddb-node");
                if node_file.exists() {
                    return Err(DoogatError::NotFound(msg));
                }
                let name = hostname::get()
                    .map(|h| h.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "unknown".to_string());
                tracing::info!(node_name = %name, "auto-registering node for first sync");
                self.register_node(&name)?;
                SyncManager::open(&self.repo)?
            }
            Err(e) => return Err(e),
        };
        mgr.sync(remote, branch, &self.index)
    }

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

    // ── Discovery / Sequences ───────────────────────────────────────────

    pub fn unlinked_mentions(&self, id: &str) -> Result<Vec<UnlinkedMention>> {
        self.ensure_fresh()?;
        self.index.unlinked_mentions(id)
    }

    pub fn suggest_links(&self, id: &str, limit: usize) -> Result<Vec<Suggestion>> {
        self.ensure_fresh()?;
        self.index.suggest_links(id, limit)
    }

    pub fn stale_doogats(&self, type_filter: Option<&str>) -> Result<Vec<StaleDoogat>> {
        self.ensure_fresh()?;
        self.index.stale_doogats(&self.repo, type_filter)
    }

    pub fn orphan_doogats(&self, type_filter: Option<&str>) -> Result<Vec<OrphanDoogat>> {
        self.ensure_fresh()?;
        self.index.orphan_doogats(type_filter)
    }

    pub fn recent_doogats(
        &self,
        days: u32,
        type_filter: Option<&str>,
    ) -> Result<Vec<RecentDoogat>> {
        self.ensure_fresh()?;
        self.index.recent_doogats(days, type_filter)
    }

    pub fn link_density(&self, type_filter: Option<&str>) -> Result<Vec<LinkDensityEntry>> {
        self.ensure_fresh()?;
        self.index.link_density(type_filter)
    }

    pub fn sequence_tree(&self, id: &str, max_depth: usize) -> Result<Vec<(SequenceNode, usize)>> {
        self.ensure_fresh()?;
        self.index.sequence_tree(id, max_depth)
    }

    pub fn sequence_breadcrumb(&self, id: &str) -> Result<Vec<SequenceNode>> {
        self.ensure_fresh()?;
        self.index.sequence_breadcrumb(id)
    }

    pub fn broken_sequences(&self) -> Result<Vec<BrokenSequence>> {
        self.ensure_fresh()?;
        self.index.broken_sequences()
    }

    pub fn sequence_info(&self, id: &str) -> Result<SequenceInfo> {
        self.ensure_fresh()?;
        self.index.sequence_info(id)
    }

    pub fn sequence_children(&self, id: &str) -> Result<Vec<SequenceNode>> {
        self.ensure_fresh()?;
        self.index.sequence_children(id)
    }

    /// Return backlink source IDs for a given doogat path/ID.
    pub fn backlink_ids(&self, id: &str) -> Result<Vec<String>> {
        self.ensure_fresh()?;
        self.index.backlinks(id)
    }

    /// Verify index is reachable.
    pub fn health_check(&self) -> Result<bool> {
        Ok(self.index.query_raw("SELECT 1").is_ok())
    }

    /// Install a bundled type definition, returning the new doogat ID.
    pub fn install_bundled_type(&self, name: &str) -> Result<String> {
        let content = crate::bundled_types::get_bundled_type(name).ok_or_else(|| {
            DoogatError::BadRequest(format!(
                "unknown bundled type \"{name}\". available: {:?}",
                crate::bundled_types::list_bundled_types()
            ))
        })?;

        let id = parser::generate_id();
        let full_content = content.replacen("---\n", &format!("---\nid: {}\n", id), 1);
        let path = format!("ddb/_typedef/{}.md", id);
        self.repo
            .commit_file(&path, &full_content, &format!("install type {name}"))?;
        let parsed = parser::parse(&full_content, &path)?;
        self.index.index_doogat(&parsed)?;

        Ok(id.to_string())
    }

    /// List all non-typedef doogat IDs.
    pub fn all_doogat_ids(&self) -> Result<Vec<String>> {
        self.ensure_fresh()?;
        let rows = self
            .index
            .query_raw("SELECT id FROM doogats WHERE path NOT LIKE 'ddb/_typedef/%'")?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.into_iter().next())
            .collect())
    }

    // ── Utility ─────────────────────────────────────────────────────────

    pub fn list_doogats(&self) -> Result<Vec<String>> {
        self.repo.list_doogats()
    }

    pub fn resolve_path(&self, id: &str) -> Result<String> {
        self.index.resolve_path(id)
    }

    pub fn head_oid(&self) -> Result<CommitHash> {
        self.repo.head_oid()
    }

    /// Commit an arbitrary file to the git repository.
    pub fn commit_file(&self, path: &str, content: &str, message: &str) -> Result<CommitHash> {
        self.repo.commit_file(path, content, message)
    }

    pub fn is_index_stale(&self) -> Result<bool> {
        self.index.is_stale(&self.repo)
    }

    pub fn load_config(&self) -> Result<crate::types::RepoConfig> {
        self.repo.load_config()
    }

    pub fn list_type_schemas(&self) -> Result<Vec<TableSchema>> {
        let rows = self
            .index
            .query_raw("SELECT path FROM doogats WHERE type = '_typedef'")?;
        let mut schemas = Vec::new();
        for row in rows {
            if let Some(path) = row.first() {
                let content = match self.repo.read_file(path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("typedef {path}: read failed: {e}");
                        continue;
                    }
                };
                let parsed = match parser::parse(&content, path) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("typedef {path}: parse failed: {e}");
                        continue;
                    }
                };
                match crate::sql_engine::schema_from_parsed(&parsed) {
                    Ok(schema) => schemas.push(schema),
                    Err(e) => {
                        tracing::warn!("typedef {path}: schema extraction failed: {e}");
                    }
                }
            }
        }
        Ok(schemas)
    }

    pub fn infer_schema(&self, name: &str) -> Result<TableSchema> {
        self.ensure_fresh()?;
        self.index.infer_schema(name, &self.repo)
    }

    pub fn rename_doogat(&self, id: &str, new_path: &str) -> Result<RenameReport> {
        let old_path = self.index.resolve_path(id)?;
        git_ops::rename_doogat(&self.repo, &self.index, &old_path, new_path)
    }

    pub fn fix_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::fix_all(&self.repo, &self.index, dry_run)
    }

    pub fn migrate_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::migrate_all(&self.repo, dry_run)
    }

    pub fn zone_migrate_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::zone_migrate_all(&self.repo, &self.index, dry_run)
    }

    pub fn resurrected_doogats(&self) -> Result<Vec<(String, String)>> {
        self.index.resurrected_doogats()
    }

    pub fn broken_backlinks(&self) -> Result<Vec<(String, String)>> {
        self.index.broken_backlinks()
    }

    pub fn backlinking_doogat_paths(&self, id: &str) -> Result<Vec<(String, String)>> {
        self.index.backlinking_doogat_paths(id)
    }

    // ── Attachments ─────────────────────────────────────────────────────

    pub fn attach_file(
        &self,
        doogat_id: &str,
        filename: &str,
        bytes: &[u8],
        mime: &str,
    ) -> Result<AttachmentInfo> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::attach_file(&self.repo, &self.index, &id, filename, bytes, mime)
    }

    pub fn detach_file(&self, doogat_id: &str, filename: &str) -> Result<()> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::detach_file(&self.repo, &self.index, &id, filename)
    }

    pub fn list_attachments(&self, doogat_id: &str) -> Result<Vec<AttachmentInfo>> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::list_attachments(&self.repo, &id)
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

    pub fn import_bundle(&self, path: &Path) -> Result<SyncReport> {
        let mut mgr = SyncManager::open(&self.repo)?;
        crate::bundle::import_bundle(&self.repo, &mut mgr, &self.index, path)
    }

    // ── NoSQL reads ─────────────────────────────────────────────────────

    #[cfg(feature = "nosql")]
    fn open_nosql(&self) -> Result<crate::nosql::RedbIndex> {
        let redb_path = self.repo_path.join(".ddb/nosql.redb");
        crate::nosql::RedbIndex::open(&redb_path)
    }

    /// Open and rebuild the NoSQL index, returning a ready-to-query handle.
    #[cfg(feature = "nosql")]
    fn open_nosql_fresh(&self) -> Result<crate::nosql::RedbIndex> {
        let ri = self.open_nosql()?;
        ri.rebuild(&self.repo)?;
        Ok(ri)
    }

    /// Get a doogat by ID from the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_get(&self, id: &str) -> Result<Option<ParsedDoogat>> {
        self.open_nosql_fresh()?.get(id)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_get(&self, _id: &str) -> Result<Option<ParsedDoogat>> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Scan by type in the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_scan_type(&self, type_name: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.scan_by_type(type_name)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_scan_type(&self, _type_name: &str) -> Result<Vec<String>> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Scan by tag in the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_scan_tag(&self, tag: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.scan_by_tag(tag)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_scan_tag(&self, _tag: &str) -> Result<Vec<String>> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Get backlinks from the NoSQL index.
    #[cfg(feature = "nosql")]
    pub fn nosql_backlinks(&self, id: &str) -> Result<Vec<String>> {
        self.open_nosql_fresh()?.backlinks(id)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_backlinks(&self, _id: &str) -> Result<Vec<String>> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }

    /// Rebuild the NoSQL index from git.
    #[cfg(feature = "nosql")]
    pub fn nosql_rebuild(&self) -> Result<usize> {
        let ri = self.open_nosql()?;
        ri.rebuild(&self.repo)
    }

    #[cfg(not(feature = "nosql"))]
    pub fn nosql_rebuild(&self) -> Result<usize> {
        Err(DoogatError::NotFound("nosql not available".into()))
    }
}

#[cfg(test)]
mod tests;
