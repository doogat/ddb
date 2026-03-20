use std::path::{Path, PathBuf};

use crate::error::{Result, ZettelError};
use crate::git_ops::{self, GitRepo};
use crate::indexer::Index;
use crate::parser;
use crate::sql_engine::{SqlEngine, SqlResult, TransactionBuffer};
use crate::sync_manager::SyncManager;
use crate::types::{
    AttachmentInfo, BrokenSequence, CommitHash, CompactOptions, CompactionReport, FixReport,
    MaintenanceReport, NodeConfig, OrphanZettel, PaginatedSearchResult, ParsedZettel, RebuildReport,
    RenameReport, SearchResult, SequenceNode, StaleZettel, Suggestion, SyncReport, TableSchema,
    UnlinkedMention, ZettelId, ZettelMeta,
};

/// Unified orchestration layer composing GitRepo, Index, and optional NoSQL
/// index into a single entry point for all high-level operations.
///
/// CLI, FFI, and server consumers delegate to `ZettelService` instead of
/// independently composing core modules. This ensures consistent behaviour
/// (e.g. NoSQL dual-write) across all entry points.
pub struct ZettelService {
    repo: GitRepo,
    index: Index,
    txn: Option<TransactionBuffer>,
    repo_path: PathBuf,
}

impl ZettelService {
    /// Open an existing ZettelDB repository.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = GitRepo::open(path)?;
        let db_dir = path.join(".zdb");
        std::fs::create_dir_all(&db_dir)?;
        let index = Index::open(&db_dir.join("index.db"))?;
        Ok(Self {
            repo,
            index,
            txn: None,
            repo_path: path.to_path_buf(),
        })
    }

    /// Initialize a new ZettelDB repository at `path` and open it.
    pub fn init(path: &Path) -> Result<Self> {
        GitRepo::init(path)?;
        Self::open(path)
    }

    /// Path to the repository root.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Borrow the underlying `GitRepo`.
    pub fn repo(&self) -> &GitRepo {
        &self.repo
    }

    /// Borrow the underlying `Index`.
    pub fn index(&self) -> &Index {
        &self.index
    }

    // ── CRUD ────────────────────────────────────────────────────────────

    /// Create a new zettel from individual fields.
    ///
    /// Generates a unique ID, determines the storage path (flat or folder),
    /// commits to git, indexes in SQLite, and dual-writes to NoSQL.
    /// Returns the new zettel ID.
    pub fn create_zettel(
        &self,
        title: &str,
        tags: &[String],
        zettel_type: Option<&str>,
        body: &str,
    ) -> Result<String> {
        let id = self.unique_id();
        let id_str = id.to_string();

        let folder = zettel_type
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let path = git_ops::zettel_path(&id_str, zettel_type, folder);

        let meta = ZettelMeta {
            id: Some(id),
            title: Some(title.to_owned()),
            date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
            zettel_type: zettel_type.map(str::to_owned),
            tags: tags.to_vec(),
            extra: Default::default(),
        };

        let parsed = ParsedZettel {
            meta,
            body: body.to_owned(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: path.clone(),
        };

        let content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &content, &format!("create zettel {id_str}"))?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);

        Ok(id_str)
    }

    /// Create a zettel from raw Markdown content (for FFI consumers).
    ///
    /// Parses the content to extract/generate an ID, determines storage path,
    /// commits, indexes, and dual-writes. Returns the zettel ID.
    pub fn create_zettel_raw(&self, content: &str, message: &str) -> Result<String> {
        let parsed = parser::parse(content, "new.md")?;
        let id = parsed
            .meta
            .id
            .as_ref()
            .map(|z| z.0.clone())
            .unwrap_or_else(|| parser::generate_id().0);

        let folder = parsed
            .meta
            .zettel_type
            .as_deref()
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let rel_path = git_ops::zettel_path(&id, parsed.meta.zettel_type.as_deref(), folder);

        self.repo.commit_file(&rel_path, content, message)?;
        let parsed = parser::parse(content, &rel_path)?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);

        Ok(id)
    }

    /// Read a zettel's raw content by ID.
    pub fn read_zettel(&self, id: &str) -> Result<String> {
        self.index.rebuild_if_stale(&self.repo)?;
        let path = self.index.resolve_path(id)?;
        self.repo.read_file(&path)
    }

    /// Update a zettel, merging provided fields into the existing content.
    pub fn update_zettel(
        &self,
        id: &str,
        title: Option<&str>,
        tags: Option<&[String]>,
        zettel_type: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        self.index.rebuild_if_stale(&self.repo)?;
        let path = self.index.resolve_path(id)?;
        let content = self.repo.read_file(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        if let Some(t) = title {
            parsed.meta.title = Some(t.to_owned());
        }
        if let Some(t) = tags {
            parsed.meta.tags = t.to_vec();
        }
        if let Some(t) = zettel_type {
            parsed.meta.zettel_type = Some(t.to_owned());
        }
        if let Some(b) = body {
            parsed.body = b.to_owned();
        }

        let new_content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &new_content, &format!("update zettel {id}"))?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);
        Ok(())
    }

    /// Update a zettel from raw content (for FFI consumers).
    pub fn update_zettel_raw(&self, id: &str, content: &str, message: &str) -> Result<()> {
        let rel_path = self.index.resolve_path(id)?;
        self.repo.commit_file(&rel_path, content, message)?;
        let parsed = parser::parse(content, &rel_path)?;
        self.index.index_zettel(&parsed)?;
        self.nosql_index_zettel(&parsed);
        Ok(())
    }

    /// Delete a zettel by ID. Returns broken backlinks `(source_id, source_path)`.
    pub fn delete_zettel(&self, id: &str) -> Result<Vec<(String, String)>> {
        self.index.rebuild_if_stale(&self.repo)?;
        let path = self.index.resolve_path(id)?;
        let broken = self.index.backlinking_zettel_paths(id)?;
        self.repo
            .delete_file(&path, &format!("delete zettel {id}"))?;
        self.index.remove_zettel(id)?;
        self.nosql_remove_zettel(id);
        Ok(broken)
    }

    // ── Search ──────────────────────────────────────────────────────────

    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.search(query)
    }

    pub fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.search_paginated(query, limit, offset)
    }

    pub fn reindex(&self) -> Result<RebuildReport> {
        self.index.rebuild(&self.repo)
    }

    pub fn rebuild_if_stale(&self) -> Result<()> {
        self.index.rebuild_if_stale(&self.repo)?;
        Ok(())
    }

    // ── SQL ─────────────────────────────────────────────────────────────

    pub fn execute_sql(&mut self, sql: &str) -> Result<SqlResult> {
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let result = engine.execute(sql).map_err(|e| {
            self.txn = engine.suspend_transaction();
            e
        })?;
        self.txn = engine.suspend_transaction();
        Ok(result)
    }

    pub fn execute_batch(&mut self, sql: &str) -> Result<Vec<SqlResult>> {
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        if let Some(buf) = self.txn.take() {
            engine.resume_transaction(buf);
        }
        let results = engine.execute_batch(sql).map_err(|e| {
            self.txn = engine.suspend_transaction();
            e
        })?;
        self.txn = engine.suspend_transaction();
        Ok(results)
    }

    pub fn begin_transaction(&mut self) -> Result<()> {
        if self.txn.is_some() {
            return Err(ZettelError::SqlEngine(
                "transaction already active".into(),
            ));
        }
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.execute("BEGIN")?;
        self.txn = engine.suspend_transaction();
        Ok(())
    }

    pub fn commit_transaction(&mut self) -> Result<()> {
        let buf = self.txn.take().ok_or_else(|| {
            ZettelError::SqlEngine("no active transaction".into())
        })?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        engine.execute("COMMIT").map_err(|e| {
            self.txn = engine.suspend_transaction();
            e
        })?;
        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<()> {
        let buf = self.txn.take().ok_or_else(|| {
            ZettelError::SqlEngine("no active transaction".into())
        })?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        engine.execute("ROLLBACK")?;
        Ok(())
    }

    // ── Sync / Compact / Maintenance ────────────────────────────────────

    pub fn sync(&self, remote: &str, branch: &str) -> Result<SyncReport> {
        let mut mgr = SyncManager::open(&self.repo)?;
        mgr.sync(remote, branch, &self.index)
    }

    pub fn compact(&self, opts: &CompactOptions) -> Result<CompactionReport> {
        let mgr = SyncManager::open(&self.repo)?;
        crate::compaction::compact(&self.repo, &mgr, opts)
    }

    pub fn run_maintenance(&self, tasks: Option<&[&str]>) -> Result<MaintenanceReport> {
        crate::maintenance::run(&self.repo.path, tasks)
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
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.unlinked_mentions(id)
    }

    pub fn suggest_links(&self, id: &str, limit: usize) -> Result<Vec<Suggestion>> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.suggest_links(id, limit)
    }

    pub fn stale_zettels(&self, type_filter: Option<&str>) -> Result<Vec<StaleZettel>> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.stale_zettels(&self.repo, type_filter)
    }

    pub fn orphan_zettels(&self, type_filter: Option<&str>) -> Result<Vec<OrphanZettel>> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.orphan_zettels(type_filter)
    }

    pub fn sequence_tree(
        &self,
        id: &str,
        max_depth: usize,
    ) -> Result<Vec<(SequenceNode, usize)>> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.sequence_tree(id, max_depth)
    }

    pub fn sequence_breadcrumb(&self, id: &str) -> Result<Vec<SequenceNode>> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.sequence_breadcrumb(id)
    }

    pub fn broken_sequences(&self) -> Result<Vec<BrokenSequence>> {
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.broken_sequences()
    }

    // ── Utility ─────────────────────────────────────────────────────────

    pub fn list_zettels(&self) -> Result<Vec<String>> {
        self.repo.list_zettels()
    }

    pub fn resolve_path(&self, id: &str) -> Result<String> {
        self.index.resolve_path(id)
    }

    pub fn head_oid(&self) -> Result<CommitHash> {
        self.repo.head_oid()
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
            .query_raw("SELECT path FROM zettels WHERE type = '_typedef'")?;
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
        self.index.rebuild_if_stale(&self.repo)?;
        self.index.infer_schema(name, &self.repo)
    }

    pub fn rename_zettel(&self, id: &str, new_path: &str) -> Result<RenameReport> {
        let old_path = self.index.resolve_path(id)?;
        git_ops::rename_zettel(&self.repo, &self.index, &old_path, new_path)
    }

    pub fn fix_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::fix_all(&self.repo, &self.index, dry_run)
    }

    pub fn migrate_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::migrate_all(&self.repo, dry_run)
    }

    pub fn resurrected_zettels(&self) -> Result<Vec<(String, String)>> {
        self.index.resurrected_zettels()
    }

    pub fn broken_backlinks(&self) -> Result<Vec<(String, String)>> {
        self.index.broken_backlinks()
    }

    pub fn backlinking_zettel_paths(&self, id: &str) -> Result<Vec<(String, String)>> {
        self.index.backlinking_zettel_paths(id)
    }

    // ── Attachments ─────────────────────────────────────────────────────

    pub fn attach_file(
        &self,
        zettel_id: &str,
        filename: &str,
        bytes: &[u8],
        mime: &str,
    ) -> Result<AttachmentInfo> {
        let id = ZettelId(zettel_id.to_owned());
        crate::attachments::attach_file(&self.repo, &self.index, &id, filename, bytes, mime)
    }

    pub fn detach_file(&self, zettel_id: &str, filename: &str) -> Result<()> {
        let id = ZettelId(zettel_id.to_owned());
        crate::attachments::detach_file(&self.repo, &self.index, &id, filename)
    }

    pub fn list_attachments(&self, zettel_id: &str) -> Result<Vec<AttachmentInfo>> {
        let id = ZettelId(zettel_id.to_owned());
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

    // ── Private helpers ─────────────────────────────────────────────────

    /// Generate a unique zettel ID, checking the filesystem for collisions.
    fn unique_id(&self) -> ZettelId {
        let zk = self.repo_path.join("zettelkasten");
        parser::generate_unique_id(|candidate| {
            let filename = format!("{candidate}.md");
            if zk.join(&filename).exists() {
                return true;
            }
            if let Ok(entries) = std::fs::read_dir(&zk) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() && entry.path().join(&filename).exists() {
                        return true;
                    }
                }
            }
            false
        })
    }

    /// Best-effort dual-write to NoSQL index.
    #[cfg(feature = "nosql")]
    fn nosql_index_zettel(&self, zettel: &ParsedZettel) {
        let redb_path = self.repo_path.join(".zdb/nosql.redb");
        if let Ok(ri) = crate::nosql::RedbIndex::open(&redb_path) {
            let _ = ri.index_zettel(zettel);
        }
    }

    #[cfg(not(feature = "nosql"))]
    fn nosql_index_zettel(&self, _zettel: &ParsedZettel) {}

    /// Best-effort removal from NoSQL index.
    #[cfg(feature = "nosql")]
    fn nosql_remove_zettel(&self, id: &str) {
        let redb_path = self.repo_path.join(".zdb/nosql.redb");
        if let Ok(ri) = crate::nosql::RedbIndex::open(&redb_path) {
            let _ = ri.remove_zettel(id);
        }
    }

    #[cfg(not(feature = "nosql"))]
    fn nosql_remove_zettel(&self, _id: &str) {}
}
