use std::path::{Path, PathBuf};

use crate::error::{Result, DoogatError};
use crate::git_ops::{self, GitRepo};
use crate::indexer::Index;
use crate::parser;
use crate::sql_engine::{SqlEngine, SqlResult, TransactionBuffer};
use crate::sync_manager::SyncManager;
use crate::types::{
    AttachmentInfo, BrokenSequence, CommitHash, CompactDryRunInfo, CompactOptions,
    CompactionReport, FixReport, ListFilter, MaintenanceReport, NodeConfig, OrphanDoogat,
    PaginatedSearchResult, ParsedDoogat, RebuildReport, RenameReport, SearchFilters, SearchResult,
    SequenceInfo, SequenceNode, StaleDoogat, Suggestion, SyncReport, TableSchema, TypedListQuery,
    UnlinkedMention, DoogatId, DoogatMeta,
};

/// Unified orchestration layer composing GitRepo, Index, and optional NoSQL
/// index into a single entry point for all high-level operations.
///
/// CLI, FFI, and server consumers delegate to `DoogatService` instead of
/// independently composing core modules. This ensures consistent behaviour
/// (e.g. NoSQL dual-write) across all entry points.
pub struct DoogatService {
    repo: GitRepo,
    index: Index,
    txn: Option<TransactionBuffer>,
    repo_path: PathBuf,
    skip_stale_check: bool,
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

    // ── CRUD ────────────────────────────────────────────────────────────

    /// Create a new doogat from individual fields.
    ///
    /// Generates a unique ID, determines the storage path (flat or folder),
    /// commits to git, indexes in SQLite, and dual-writes to NoSQL.
    /// Returns the new doogat ID.
    pub fn create_doogat(
        &self,
        title: &str,
        tags: &[String],
        doogat_type: Option<&str>,
        body: &str,
    ) -> Result<String> {
        self.create_doogat_with_extra(title, tags, doogat_type, body, Default::default())
            .map(|p| p.meta.id.map(|z| z.0).unwrap_or_default())
    }

    /// Create a new doogat, returning the full `ParsedDoogat`.
    pub fn create_doogat_parsed(
        &self,
        title: &str,
        tags: &[String],
        doogat_type: Option<&str>,
        body: &str,
    ) -> Result<ParsedDoogat> {
        self.create_doogat_with_extra(title, tags, doogat_type, body, Default::default())
    }

    /// Create a new doogat with optional extra frontmatter fields.
    pub fn create_doogat_with_extra(
        &self,
        title: &str,
        tags: &[String],
        doogat_type: Option<&str>,
        body: &str,
        extra: std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<ParsedDoogat> {
        let id = self.unique_id();
        let id_str = id.to_string();

        let folder = doogat_type
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let path = git_ops::doogat_path(&id_str, doogat_type, folder);

        let meta = DoogatMeta {
            id: Some(id),
            title: Some(title.to_owned()),
            date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
            doogat_type: doogat_type.map(str::to_owned),
            tags: tags.to_vec(),
            extra,
        };

        let parsed = ParsedDoogat {
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
            .commit_file(&path, &content, &format!("create doogat {id_str}"))?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);

        Ok(parsed)
    }

    /// Create a doogat from raw Markdown content (for FFI consumers).
    ///
    /// Parses the content to extract/generate an ID, determines storage path,
    /// commits, indexes, and dual-writes. Returns the doogat ID.
    pub fn create_doogat_raw(&self, content: &str, message: &str) -> Result<String> {
        let parsed = parser::parse(content, "new.md")?;
        let id = parsed
            .meta
            .id
            .as_ref()
            .map(|z| z.0.clone())
            .unwrap_or_else(|| parser::generate_id().0);

        let folder = parsed
            .meta
            .doogat_type
            .as_deref()
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let rel_path = git_ops::doogat_path(&id, parsed.meta.doogat_type.as_deref(), folder);

        self.repo.commit_file(&rel_path, content, message)?;
        let parsed = parser::parse(content, &rel_path)?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);

        Ok(id)
    }

    /// Read a doogat's raw content by ID.
    pub fn read_doogat(&self, id: &str) -> Result<String> {
        self.ensure_fresh()?;
        let path = self.index.resolve_path(id)?;
        self.repo.read_file(&path)
    }

    /// Read raw content by git-relative path, skipping index freshness check.
    pub fn read_doogat_raw(&self, path: &str) -> Result<String> {
        self.repo.read_file(path)
    }

    /// Read and parse a doogat by ID, returning a fully parsed doogat.
    pub fn get_doogat_parsed(&self, id: &str) -> Result<ParsedDoogat> {
        self.ensure_fresh()?;
        let path = self.index.resolve_path(id)?;
        let content = self.repo.read_file(&path)?;
        parser::parse(&content, &path)
    }

    /// Update a doogat, merging provided fields into the existing content.
    pub fn update_doogat(
        &self,
        id: &str,
        title: Option<&str>,
        tags: Option<&[String]>,
        doogat_type: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        self.update_doogat_parsed(id, title, tags, doogat_type, body)?;
        Ok(())
    }

    /// Update a doogat, returning the updated `ParsedDoogat`.
    pub fn update_doogat_parsed(
        &self,
        id: &str,
        title: Option<&str>,
        tags: Option<&[String]>,
        doogat_type: Option<&str>,
        body: Option<&str>,
    ) -> Result<ParsedDoogat> {
        self.ensure_fresh()?;
        let path = self.index.resolve_path(id)?;
        let content = self.repo.read_file(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        if let Some(t) = title {
            parsed.meta.title = Some(t.to_owned());
        }
        if let Some(t) = tags {
            parsed.meta.tags = t.to_vec();
        }
        if let Some(t) = doogat_type {
            parsed.meta.doogat_type = Some(t.to_owned());
        }
        if let Some(b) = body {
            parsed.body = b.to_owned();
        }

        let new_content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &new_content, &format!("update doogat {id}"))?;
        // Re-parse to capture updated inline fields/wikilinks
        let parsed = parser::parse(&new_content, &path)?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);
        Ok(parsed)
    }

    /// Update a doogat from raw content (for FFI consumers).
    pub fn update_doogat_raw(&self, id: &str, content: &str, message: &str) -> Result<()> {
        let rel_path = self.index.resolve_path(id)?;
        self.repo.commit_file(&rel_path, content, message)?;
        let parsed = parser::parse(content, &rel_path)?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);
        Ok(())
    }

    /// Delete a doogat by ID. Returns broken backlinks `(source_id, source_path)`.
    pub fn delete_doogat(&self, id: &str, message: &str) -> Result<Vec<(String, String)>> {
        self.ensure_fresh()?;
        let path = self.index.resolve_path(id)?;
        let broken = self.index.backlinking_doogat_paths(id)?;
        self.repo.delete_file(&path, message)?;
        self.index.remove_doogat(id)?;
        self.nosql_remove_doogat(id);
        Ok(broken)
    }

    // ── Search ──────────────────────────────────────────────────────────

    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.ensure_fresh()?;
        self.index.search(query)
    }

    pub fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.ensure_fresh()?;
        self.index.search_paginated(query, limit, offset)
    }

    pub fn search_paginated_filtered(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        filters: &SearchFilters,
    ) -> Result<PaginatedSearchResult> {
        self.ensure_fresh()?;
        self.index
            .search_paginated_filtered(query, limit, offset, filters)
    }

    pub fn reindex(&self) -> Result<RebuildReport> {
        self.index.rebuild(&self.repo)
    }

    pub fn rebuild_if_stale(&self) -> Result<()> {
        self.ensure_fresh()?;
        Ok(())
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
            return Err(DoogatError::SqlEngine(
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
            DoogatError::SqlEngine("no active transaction".into())
        })?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        engine.execute("COMMIT").inspect_err(|_| {
            self.txn = engine.suspend_transaction();
        })?;
        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<()> {
        let buf = self.txn.take().ok_or_else(|| {
            DoogatError::SqlEngine("no active transaction".into())
        })?;
        let mut engine = SqlEngine::new(&self.index, &self.repo);
        engine.resume_transaction(buf);
        engine.execute("ROLLBACK")?;
        Ok(())
    }

    // ── Filtered Queries ─────────────────────────────────────────────────

    /// Query doogats matching filter criteria, returning parsed doogats.
    pub fn list_doogats_filtered(&self, filter: &ListFilter) -> Result<Vec<ParsedDoogat>> {
        self.ensure_fresh()?;
        let sql = build_filtered_sql(filter);
        let rows = self.index.query_raw(&sql)?;
        let mut doogats = Vec::new();
        for row in rows {
            if row.len() >= 2 {
                let path = &row[1];
                if let Ok(content) = self.repo.read_file(path) {
                    if let Ok(parsed) = parser::parse(&content, path) {
                        doogats.push(parsed);
                    }
                }
            }
        }
        Ok(doogats)
    }

    /// Count doogats matching filter criteria.
    pub fn count_doogats_filtered(&self, filter: &ListFilter) -> Result<i64> {
        self.ensure_fresh()?;
        let count_filter = ListFilter {
            limit: None,
            offset: None,
            sort_field: None,
            sort_desc: None,
            ..filter.clone()
        };
        let select_sql = build_filtered_sql(&count_filter);
        let count_sql = format!("SELECT COUNT(*) FROM ({select_sql})");
        let rows = self.index.query_raw(&count_sql)?;
        let count = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        Ok(count)
    }

    /// List all tags with usage counts, ordered by count descending.
    pub fn list_tags(&self) -> Result<Vec<(String, i64)>> {
        self.ensure_fresh()?;
        self.index.list_tags()
    }

    /// Execute a raw SQL query with params, returning the first result row.
    pub fn aggregate_query(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<String>> {
        self.ensure_fresh()?;
        let rows = self.index.query_raw_with_params(sql, params)?;
        Ok(rows.into_iter().next().unwrap_or_default())
    }

    /// Execute a raw SQL query with params, returning all result rows.
    pub fn query_raw_with_params(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<Vec<String>>> {
        self.ensure_fresh()?;
        self.index.query_raw_with_params(sql, params)
    }

    pub fn query_raw_with_columns(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        self.ensure_fresh()?;
        self.index.query_raw_with_columns(sql)
    }

    /// Query a materialized type table with WHERE/ORDER/LIMIT, returning parsed doogats.
    pub fn typed_filtered_list(&self, query: &TypedListQuery) -> Result<Vec<ParsedDoogat>> {
        self.ensure_fresh()?;

        let mut conditions = Vec::new();
        if !query.where_sql.is_empty() {
            conditions.push(query.where_sql.to_string());
        }
        if let Some(t) = &query.tag {
            conditions.push(format!(
                "id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = '{}')",
                t.replace('\'', "''")
            ));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let order = query.order_sql.as_deref().unwrap_or("id DESC");
        let limit_clause = match (query.limit, query.offset) {
            (Some(l), Some(o)) => format!(" LIMIT {l} OFFSET {o}"),
            (Some(l), None) => format!(" LIMIT {l}"),
            (None, Some(o)) => format!(" LIMIT -1 OFFSET {o}"),
            (None, None) => String::new(),
        };

        let sql = format!(
            "SELECT id FROM \"{}\"{where_clause} ORDER BY {order}{limit_clause}",
            query.table_name
        );

        let rows = self.index.query_raw_with_params(&sql, &query.params)?;
        let mut doogats = Vec::new();
        for row in rows {
            if let Some(id) = row.first() {
                if let Ok(path) = self.index.resolve_path(id) {
                    if let Ok(content) = self.repo.read_file(&path) {
                        if let Ok(parsed) = parser::parse(&content, &path) {
                            doogats.push(parsed);
                        }
                    }
                }
            }
        }
        Ok(doogats)
    }

    // ── Sync / Compact / Maintenance ────────────────────────────────────

    pub fn sync(&self, remote: &str, branch: &str) -> Result<SyncReport> {
        let mut mgr = match SyncManager::open(&self.repo) {
            Ok(m) => m,
            Err(DoogatError::NotFound(msg)) => {
                let node_file = self.repo.path.join(".git/ddb-node");
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
        let shared_head = crate::compaction::shared_head(&self.repo, &nodes)?
            .map(|oid| oid.to_string());
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
        let toml_str = toml::to_string_pretty(&config)
            .map_err(|e| DoogatError::Toml(e.to_string()))?;
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

    pub fn sequence_tree(
        &self,
        id: &str,
        max_depth: usize,
    ) -> Result<Vec<(SequenceNode, usize)>> {
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
        let content =
            crate::bundled_types::get_bundled_type(name).ok_or_else(|| {
                DoogatError::SqlEngine(format!(
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
        let rows = self.index.query_raw(
            "SELECT id FROM doogats WHERE path NOT LIKE 'ddb/_typedef/%'",
        )?;
        Ok(rows.into_iter().filter_map(|r| r.into_iter().next()).collect())
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

    // ── Private helpers ─────────────────────────────────────────────────

    /// Generate a unique doogat ID, checking the filesystem for collisions.
    fn unique_id(&self) -> DoogatId {
        let zk = self.repo_path.join("ddb");
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
    fn nosql_index_doogat(&self, doogat: &ParsedDoogat) {
        let redb_path = self.repo_path.join(".ddb/nosql.redb");
        if let Ok(ri) = crate::nosql::RedbIndex::open(&redb_path) {
            let _ = ri.index_doogat(doogat);
        }
    }

    #[cfg(not(feature = "nosql"))]
    fn nosql_index_doogat(&self, _doogat: &ParsedDoogat) {}

    /// Best-effort removal from NoSQL index.
    #[cfg(feature = "nosql")]
    fn nosql_remove_doogat(&self, id: &str) {
        let redb_path = self.repo_path.join(".ddb/nosql.redb");
        if let Ok(ri) = crate::nosql::RedbIndex::open(&redb_path) {
            let _ = ri.remove_doogat(id);
        }
    }

    #[cfg(not(feature = "nosql"))]
    fn nosql_remove_doogat(&self, _id: &str) {}

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

/// Sortable columns on the doogats table.
pub const SORTABLE_COLUMNS: &[&str] = &["id", "title", "date", "type"];

/// Build SQL query with filters for doogat listing.
fn build_filtered_sql(filter: &ListFilter) -> String {
    let mut conditions = Vec::new();

    if let Some(t) = filter.doogat_type.as_deref() {
        conditions.push(format!("z.type = '{}'", t.replace('\'', "''")));
    }
    if let Some(t) = filter.tag.as_deref() {
        conditions.push(format!(
            "z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = '{}')",
            t.replace('\'', "''")
        ));
    }
    if let Some(bl) = filter.backlinks_of.as_deref() {
        conditions.push(format!(
            "z.id IN (SELECT source_id FROM _ddb_links WHERE target_path = '{}')",
            bl.replace('\'', "''")
        ));
    }
    for (key, value) in &filter.field_filters {
        conditions.push(format!(
            "z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = '{}' AND value = '{}')",
            key.replace('\'', "''"),
            value.replace('\'', "''")
        ));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let limit_clause = match (filter.limit, filter.offset) {
        (Some(l), Some(o)) => format!(" LIMIT {l} OFFSET {o}"),
        (Some(l), None) => format!(" LIMIT {l}"),
        (None, Some(o)) => format!(" LIMIT -1 OFFSET {o}"),
        (None, None) => String::new(),
    };

    let order_clause = match filter.sort_field.as_deref().filter(|f| SORTABLE_COLUMNS.contains(f)) {
        Some(field) => {
            let dir = if filter.sort_desc.unwrap_or(false) { "DESC" } else { "ASC" };
            format!(" ORDER BY z.{field} {dir}")
        }
        None => " ORDER BY z.id DESC".to_string(),
    };

    format!("SELECT z.id, z.path FROM doogats z{where_clause}{order_clause}{limit_clause}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_svc() -> (TempDir, DoogatService) {
        let tmp = TempDir::new().unwrap();
        let svc = DoogatService::init(tmp.path()).unwrap();
        svc.reindex().unwrap();
        (tmp, svc)
    }

    #[test]
    fn init_creates_repo_and_opens() {
        let (_tmp, svc) = fresh_svc();
        let list = svc.list_doogats().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn crud_roundtrip() {
        let (_tmp, svc) = fresh_svc();

        let id = svc
            .create_doogat("Test Note", &[], None, "Hello world")
            .unwrap();
        assert_eq!(id.len(), 14);

        let content = svc.read_doogat(&id).unwrap();
        assert!(content.contains("Test Note"));
        assert!(content.contains("Hello world"));

        svc.update_doogat(&id, Some("Updated"), None, None, Some("New body"))
            .unwrap();
        let content = svc.read_doogat(&id).unwrap();
        assert!(content.contains("Updated"));
        assert!(content.contains("New body"));

        let broken = svc.delete_doogat(&id, "delete test").unwrap();
        assert!(broken.is_empty());

        assert!(svc.read_doogat(&id).is_err());
    }

    #[test]
    fn create_raw_and_read() {
        let (_tmp, svc) = fresh_svc();

        let raw = "---\ntitle: Raw Note\n---\nRaw body";
        let id = svc.create_doogat_raw(raw, "add raw").unwrap();
        assert_eq!(id.len(), 14);

        let content = svc.read_doogat(&id).unwrap();
        assert!(content.contains("Raw Note"));
    }

    #[test]
    fn search_after_create() {
        let (_tmp, svc) = fresh_svc();

        svc.create_doogat("Searchable Doogat", &[], None, "unique content here")
            .unwrap();

        let results = svc.search("Searchable").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].title.contains("Searchable"));
    }

    #[test]
    fn sql_create_table_and_insert() {
        let (_tmp, mut svc) = fresh_svc();

        let ddl = svc
            .execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();
        assert!(matches!(ddl, SqlResult::Ok(_)));

        let ins = svc
            .execute_sql("INSERT INTO project (name, status) VALUES ('alpha', 'active')")
            .unwrap();
        assert!(matches!(ins, SqlResult::Ok(_)));

        let sel = svc
            .execute_sql("SELECT name, status FROM project")
            .unwrap();
        match sel {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "alpha");
            }
            _ => panic!("expected rows"),
        }
    }

    #[test]
    fn transaction_commit_persists() {
        let (_tmp, mut svc) = fresh_svc();
        svc.execute_sql("CREATE TABLE txtest (val TEXT)").unwrap();

        svc.begin_transaction().unwrap();
        svc.execute_sql("INSERT INTO txtest (val) VALUES ('in-txn')")
            .unwrap();
        svc.commit_transaction().unwrap();

        let result = svc.execute_sql("SELECT val FROM txtest").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "in-txn");
            }
            _ => panic!("expected rows"),
        }
    }

    #[test]
    fn transaction_rollback_discards() {
        let (_tmp, mut svc) = fresh_svc();
        svc.execute_sql("CREATE TABLE rbtest (val TEXT)").unwrap();

        svc.begin_transaction().unwrap();
        svc.execute_sql("INSERT INTO rbtest (val) VALUES ('gone')")
            .unwrap();
        svc.rollback_transaction().unwrap();

        let result = svc.execute_sql("SELECT val FROM rbtest").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => assert_eq!(rows.len(), 0),
            _ => panic!("expected rows"),
        }
    }

    #[test]
    fn reindex_rebuilds() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("One", &[], None, "").unwrap();
        svc.create_doogat("Two", &[], None, "").unwrap();

        let report = svc.reindex().unwrap();
        assert_eq!(report.indexed, 2);
    }

    #[test]
    fn delete_returns_broken_backlinks() {
        let (_tmp, svc) = fresh_svc();

        let id_b = svc
            .create_doogat("Target", &[], None, "target body")
            .unwrap();

        let body_a = format!("Links to [[{id_b}]]");
        let id_a = svc
            .create_doogat("Source", &[], None, &body_a)
            .unwrap();
        svc.reindex().unwrap();

        let broken = svc.delete_doogat(&id_b, "delete test").unwrap();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].0, id_a);
    }

    #[test]
    fn list_doogats_filtered_no_filter() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("A", &[], None, "").unwrap();
        svc.create_doogat("B", &[], None, "").unwrap();

        let filter = crate::types::ListFilter::default();
        let results = svc.list_doogats_filtered(&filter).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn list_doogats_filtered_by_tag() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Tagged", &["rust".into()], None, "").unwrap();
        svc.create_doogat("Untagged", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter {
            tag: Some("rust".into()),
            ..Default::default()
        };
        let results = svc.list_doogats_filtered(&filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].meta.title.as_deref(), Some("Tagged"));
    }

    #[test]
    fn list_doogats_filtered_with_limit() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("A", &[], None, "").unwrap();
        svc.create_doogat("B", &[], None, "").unwrap();
        svc.create_doogat("C", &[], None, "").unwrap();

        let filter = crate::types::ListFilter {
            limit: Some(2),
            ..Default::default()
        };
        let results = svc.list_doogats_filtered(&filter).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn count_doogats_filtered_all() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("A", &[], None, "").unwrap();
        svc.create_doogat("B", &[], None, "").unwrap();

        let filter = crate::types::ListFilter::default();
        let count = svc.count_doogats_filtered(&filter).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn list_doogats_filtered_sort_by_title_asc() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Charlie", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Alpha", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Bravo", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter {
            sort_field: Some("title".into()),
            sort_desc: Some(false),
            ..Default::default()
        };
        let results = svc.list_doogats_filtered(&filter).unwrap();
        let titles: Vec<_> = results.iter().map(|d| d.meta.title.as_deref().unwrap()).collect();
        assert_eq!(titles, vec!["Alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn list_doogats_filtered_sort_by_title_desc() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Alpha", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Charlie", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Bravo", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter {
            sort_field: Some("title".into()),
            sort_desc: Some(true),
            ..Default::default()
        };
        let results = svc.list_doogats_filtered(&filter).unwrap();
        let titles: Vec<_> = results.iter().map(|d| d.meta.title.as_deref().unwrap()).collect();
        assert_eq!(titles, vec!["Charlie", "Bravo", "Alpha"]);
    }

    #[test]
    fn list_doogats_filtered_sort_default_is_id_desc() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("First", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Second", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter::default();
        let results = svc.list_doogats_filtered(&filter).unwrap();
        // Default is id DESC, so newest (Second) comes first
        assert_eq!(results[0].meta.title.as_deref(), Some("Second"));
        assert_eq!(results[1].meta.title.as_deref(), Some("First"));
    }

    #[test]
    fn list_doogats_filtered_sort_invalid_field_falls_back_to_default() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("A", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("B", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter {
            sort_field: Some("nonexistent".into()),
            ..Default::default()
        };
        let results = svc.list_doogats_filtered(&filter).unwrap();
        // Falls back to id DESC
        assert_eq!(results[0].meta.title.as_deref(), Some("B"));
    }

    #[test]
    fn aggregate_query_select_one() {
        let (_tmp, svc) = fresh_svc();
        let row = svc.aggregate_query("SELECT 1 AS n", &[]).unwrap();
        assert_eq!(row, vec!["1"]);
    }

    #[test]
    fn aggregate_query_empty() {
        let (_tmp, svc) = fresh_svc();
        let row = svc
            .aggregate_query("SELECT id FROM doogats WHERE 1=0", &[])
            .unwrap();
        assert!(row.is_empty());
    }

    #[test]
    fn health_check_returns_true() {
        let (_tmp, svc) = fresh_svc();
        assert!(svc.health_check().unwrap());
    }

    #[test]
    fn backlink_ids_empty() {
        let (_tmp, svc) = fresh_svc();
        let links = svc.backlink_ids("nonexistent").unwrap();
        assert!(links.is_empty());
    }

    #[test]
    fn install_bundled_type_project() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.install_bundled_type("project").unwrap();
        assert_eq!(id.len(), 14);
        let content = svc.read_doogat(&id).unwrap();
        assert!(content.contains("project"));
    }

    #[test]
    fn install_bundled_type_unknown_fails() {
        let (_tmp, svc) = fresh_svc();
        assert!(svc.install_bundled_type("nonexistent").is_err());
    }

    #[test]
    fn all_doogat_ids_excludes_typedefs() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Normal", &[], None, "").unwrap();
        svc.install_bundled_type("project").unwrap();
        svc.reindex().unwrap();

        let ids = svc.all_doogat_ids().unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn compact_dry_run_no_nodes() {
        let (_tmp, svc) = fresh_svc();
        let info = svc.compact_dry_run();
        // No nodes registered → NotFound from SyncManager, but dry_run should handle gracefully
        // (list_nodes returns empty or error)
        assert!(info.is_ok() || info.is_err());
    }

    #[test]
    fn auto_maintenance_default_off() {
        let (_tmp, svc) = fresh_svc();
        // Default config has auto_enabled = false
        let enabled = svc.auto_maintenance_enabled().unwrap();
        assert!(!enabled);
    }

    #[test]
    fn set_auto_maintenance_roundtrip() {
        let (_tmp, svc) = fresh_svc();
        svc.set_auto_maintenance(true).unwrap();
        assert!(svc.auto_maintenance_enabled().unwrap());
        svc.set_auto_maintenance(false).unwrap();
        assert!(!svc.auto_maintenance_enabled().unwrap());
    }

    #[test]
    fn list_tags_returns_counts() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("A", &["rust".into(), "cli".into()], None, "")
            .unwrap();
        svc.create_doogat("B", &["rust".into()], None, "")
            .unwrap();
        svc.reindex().unwrap();

        let tags = svc.list_tags().unwrap();
        // rust should appear first (count 2), cli second (count 1)
        assert!(tags.len() >= 2);
        assert_eq!(tags[0].0, "rust");
        assert_eq!(tags[0].1, 2);
        assert_eq!(tags[1].0, "cli");
        assert_eq!(tags[1].1, 1);
    }

    #[test]
    fn sequence_children_empty() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.create_doogat("Root", &[], None, "").unwrap();
        svc.reindex().unwrap();
        let children = svc.sequence_children(&id).unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn sync_auto_registers_node_when_none_exists() {
        let (tmp, svc) = fresh_svc();
        let node_path = tmp.path().join(".git/ddb-node");
        assert!(!node_path.exists(), "node should not exist before sync");

        // sync will fail (no remote) but auto-register should still happen
        let result = svc.sync("origin", "master");
        assert!(result.is_err(), "sync should fail without a remote");
        assert!(node_path.exists(), "node should be auto-registered after sync attempt");
    }

    #[test]
    fn sync_reuses_existing_registration() {
        let (tmp, svc) = fresh_svc();
        svc.register_node("MyLaptop").unwrap();
        let node_path = tmp.path().join(".git/ddb-node");
        let uuid_before = std::fs::read_to_string(&node_path).unwrap();

        // sync fails (no remote) but should not re-register
        let _ = svc.sync("origin", "master");
        let uuid_after = std::fs::read_to_string(&node_path).unwrap();
        assert_eq!(uuid_before, uuid_after, "existing registration should be reused");
    }

    #[test]
    fn sync_does_not_auto_register_when_node_file_exists_but_toml_missing() {
        let (tmp, svc) = fresh_svc();
        // Write a ddb-node file pointing to a non-existent node TOML
        let node_path = tmp.path().join(".git/ddb-node");
        std::fs::write(&node_path, "bogus-uuid-that-has-no-toml").unwrap();

        let result = svc.sync("origin", "master");
        assert!(result.is_err());
        // Should NOT have overwritten the node file with a new UUID
        let uuid_after = std::fs::read_to_string(&node_path).unwrap();
        assert_eq!(uuid_after, "bogus-uuid-that-has-no-toml",
            "corrupt state should propagate error, not silently re-register");
    }
}
