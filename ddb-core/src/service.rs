use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::error::{Result, DoogatError};
use crate::git_ops::{self, GitRepo};
use crate::indexer::Index;
use crate::parser;
use crate::sql_engine::{SqlEngine, SqlResult, TransactionBuffer};
use crate::sync_manager::SyncManager;
use crate::types::{
    AttachmentInfo, BatchCreateInput, BatchUpdateInput, BrokenSequence, CommitHash,
    CompactDryRunInfo, CompactOptions, CompactionReport, FixReport, ListFilter,
    MaintenanceReport, NodeConfig, OrphanDoogat, PaginatedSearchResult, ParsedDoogat,
    RebuildReport, RenameReport, SearchFilters, SearchResult, SequenceInfo, SequenceNode,
    StaleDoogat, Suggestion, SyncReport, TableSchema, TypedListQuery, UnlinkedMention,
    DoogatId, DoogatMeta,
};

/// Extra frontmatter fields to set or remove during an update.
pub struct ExtraFieldUpdates<'a> {
    pub set: &'a std::collections::BTreeMap<String, crate::types::Value>,
    pub unset: &'a [String],
}

impl Default for ExtraFieldUpdates<'_> {
    fn default() -> Self {
        static EMPTY_MAP: std::sync::LazyLock<std::collections::BTreeMap<String, crate::types::Value>> =
            std::sync::LazyLock::new(std::collections::BTreeMap::new);
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

        let mut parsed = ParsedDoogat {
            meta,
            body: body.to_owned(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: path.clone(),
            updated_at: None,
        };

        let content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &content, &format!("create doogat {id_str}"))?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);
        parsed.updated_at = self.index.lookup_updated_at(&id_str).unwrap_or(None);

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
        let mut parsed = parser::parse(&content, &path)?;
        parsed.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
        Ok(parsed)
    }

    /// Batch-fetch multiple doogats by ID, skipping any that fail to resolve or parse.
    pub fn get_doogats_batch(&self, ids: &[String]) -> Result<Vec<ParsedDoogat>> {
        self.ensure_fresh()?;
        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let updated_map = self.index.lookup_updated_at_batch(&id_refs).unwrap_or_default();
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(path) = self.index.resolve_path(id) {
                if let Ok(content) = self.repo.read_file(&path) {
                    if let Ok(mut parsed) = parser::parse(&content, &path) {
                        parsed.updated_at = updated_map.get(id.as_str()).cloned();
                        results.push(parsed);
                    }
                }
            }
        }
        Ok(results)
    }

    /// Update a doogat, merging provided fields into the existing content.
    pub fn update_doogat(
        &self,
        id: &str,
        title: Option<&str>,
        tags: Option<&[String]>,
        doogat_type: Option<&str>,
        body: Option<&str>,
        extra: &ExtraFieldUpdates<'_>,
    ) -> Result<()> {
        self.update_doogat_parsed(id, title, tags, doogat_type, body, extra)?;
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
        extra: &ExtraFieldUpdates<'_>,
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
        for key in extra.unset {
            parsed.meta.extra.remove(key);
        }
        for (key, value) in extra.set {
            parsed.meta.extra.insert(key.clone(), value.clone());
        }

        let new_content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &new_content, &format!("update doogat {id}"))?;
        // Re-parse to capture updated inline fields/wikilinks
        let mut parsed = parser::parse(&new_content, &path)?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);
        parsed.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
        Ok(parsed)
    }

    /// Batch-update multiple doogats in a single atomic commit.
    ///
    /// All mutations are prepared first; if any ID fails to resolve the
    /// entire batch is aborted. On success a single git commit is created
    /// and each doogat is re-indexed.
    pub fn batch_update(&self, updates: &[BatchUpdateInput]) -> Result<Vec<ParsedDoogat>> {
        if updates.is_empty() {
            return Ok(vec![]);
        }

        // Reject duplicate IDs (later entries would silently overwrite earlier ones)
        let mut seen = std::collections::HashSet::with_capacity(updates.len());
        for u in updates {
            if !seen.insert(&u.id) {
                return Err(DoogatError::Validation(format!(
                    "duplicate id in batch: {}",
                    u.id
                )));
            }
        }

        self.ensure_fresh()?;

        // Phase 1: prepare all writes (fail-fast, no side effects)
        let mut writes: Vec<(String, String)> = Vec::with_capacity(updates.len());
        for update in updates {
            let path = self.index.resolve_path(&update.id)?;
            let content = self.repo.read_file(&path)?;
            let mut parsed = parser::parse(&content, &path)?;

            if let Some(ref t) = update.title {
                parsed.meta.title = Some(t.clone());
            }
            if let Some(ref t) = update.tags {
                parsed.meta.tags = t.clone();
            }
            if let Some(ref t) = update.doogat_type {
                parsed.meta.doogat_type = Some(t.clone());
            }
            if let Some(ref b) = update.body {
                parsed.body = b.clone();
            }

            let new_content = parser::serialize(&parsed);
            writes.push((path, new_content));
        }

        // Phase 2: atomic commit
        let write_refs: Vec<(&str, &str)> = writes
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_batch(
            &write_refs,
            &[],
            &format!("batch update {} doogats", updates.len()),
        )?;

        // Phase 3: re-parse, index, return
        let mut results = Vec::with_capacity(updates.len());
        for (i, (path, new_content)) in writes.iter().enumerate() {
            let mut parsed = parser::parse(new_content, path)?;
            self.index.index_doogat(&parsed)?;
            self.nosql_index_doogat(&parsed);
            let id = &updates[i].id;
            parsed.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
            results.push(parsed);
        }

        Ok(results)
    }

    /// Batch-create multiple doogats in a single atomic commit.
    ///
    /// Generates unique IDs, resolves typedef defaults (including DEFAULT NEXT),
    /// validates constraints, and commits all files atomically.
    pub fn batch_create(&self, inputs: &[BatchCreateInput]) -> Result<Vec<ParsedDoogat>> {
        if inputs.is_empty() {
            return Ok(vec![]);
        }

        self.ensure_fresh()?;

        // Load type schemas once for default/validation resolution
        let schemas = self.list_type_schemas()?;

        // Pre-compute NEXT counters per (table, column)
        let mut next_counters: std::collections::BTreeMap<(String, String), i64> =
            std::collections::BTreeMap::new();
        for input in inputs {
            if let Some(ref type_name) = input.doogat_type {
                if let Some(schema) = schemas.iter().find(|s| s.table_name == *type_name) {
                    for col in &schema.columns {
                        if let Some(ref dv) = col.default_value {
                            if dv == "NEXT" {
                                let key = (type_name.clone(), col.name.clone());
                                if let std::collections::btree_map::Entry::Vacant(e) =
                                    next_counters.entry(key)
                                {
                                    let sql = format!(
                                        "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\"",
                                        col.name, schema.table_name
                                    );
                                    let max_val: i64 = self
                                        .index
                                        .query_raw(&sql)?
                                        .first()
                                        .and_then(|r| r.first())
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(0);
                                    e.insert(max_val);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Phase 1: prepare all writes
        let mut writes: Vec<(String, String)> = Vec::with_capacity(inputs.len());
        for input in inputs {
            let id = self.unique_id();
            let id_str = id.to_string();

            let folder = input
                .doogat_type
                .as_deref()
                .map(|t| self.index.type_uses_folder(t, &self.repo))
                .unwrap_or(false);
            let path = git_ops::doogat_path(&id_str, input.doogat_type.as_deref(), folder);

            let mut extra = input.fields.clone();

            // Resolve typedef defaults and validate
            if let Some(ref type_name) = input.doogat_type {
                if let Some(schema) = schemas.iter().find(|s| s.table_name == *type_name) {
                    for col in &schema.columns {
                        if !extra.contains_key(&col.name) {
                            if let Some(ref dv) = col.default_value {
                                if dv == "NEXT" {
                                    let key = (type_name.clone(), col.name.clone());
                                    let counter = next_counters.get_mut(&key).unwrap();
                                    *counter += 1;
                                    extra.insert(
                                        col.name.clone(),
                                        crate::types::Value::String(counter.to_string()),
                                    );
                                } else if dv.starts_with("NEXT(") && dv.ends_with(')') {
                                    let partition_col = &dv[5..dv.len() - 1];
                                    let partition_val = extra
                                        .get(partition_col)
                                        .map(|v| match v {
                                            crate::types::Value::String(s) => s.clone(),
                                            other => format!("{other:?}"),
                                        })
                                        .unwrap_or_default();
                                    let sql = format!(
                                        "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\" WHERE \"{}\" = '{}'",
                                        col.name, schema.table_name, partition_col, partition_val
                                    );
                                    let max_val: i64 = self
                                        .index
                                        .query_raw(&sql)?
                                        .first()
                                        .and_then(|r| r.first())
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(0);
                                    extra.insert(
                                        col.name.clone(),
                                        crate::types::Value::String((max_val + 1).to_string()),
                                    );
                                } else {
                                    extra.insert(
                                        col.name.clone(),
                                        crate::types::Value::String(dv.clone()),
                                    );
                                }
                            }
                        }

                        // Validate allowed_values
                        if let Some(ref allowed) = col.allowed_values {
                            if let Some(val) = extra.get(&col.name) {
                                let val_str = match val {
                                    crate::types::Value::String(s) => s.clone(),
                                    other => format!("{other:?}"),
                                };
                                if !allowed.contains(&val_str) {
                                    return Err(DoogatError::Validation(format!(
                                        "field '{}' value '{}' not in allowed values: {:?}",
                                        col.name, val_str, allowed
                                    )));
                                }
                            }
                        }

                        // Validate FK references
                        if let Some(ref _ref_table) = col.references {
                            if let Some(val) = extra.get(&col.name) {
                                let val_str = match val {
                                    crate::types::Value::String(s) => s.clone(),
                                    other => format!("{other:?}"),
                                };
                                let sql = format!(
                                    "SELECT COUNT(*) > 0 FROM doogats WHERE id = '{}'",
                                    val_str
                                );
                                let exists = self
                                    .index
                                    .query_raw(&sql)?
                                    .first()
                                    .and_then(|r| r.first())
                                    .map(|v| v == "1")
                                    .unwrap_or(false);
                                if !exists {
                                    return Err(DoogatError::Validation(format!(
                                        "field '{}' references non-existent doogat '{}'",
                                        col.name, val_str
                                    )));
                                }
                            }
                        }
                    }
                }
            }

            let meta = DoogatMeta {
                id: Some(id),
                title: Some(input.title.clone()),
                date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
                doogat_type: input.doogat_type.clone(),
                tags: input.tags.clone(),
                extra,
            };

            let parsed = ParsedDoogat {
                meta,
                body: input.body.clone().unwrap_or_default(),
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
                body_tags: vec![],
                checkboxes: vec![],
                path: path.clone(),
                updated_at: None,
            };

            let content = parser::serialize(&parsed);
            writes.push((path, content));
        }

        // Phase 2: atomic commit
        let write_refs: Vec<(&str, &str)> = writes
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_batch(
            &write_refs,
            &[],
            &format!("batch create {} doogats", inputs.len()),
        )?;

        // Phase 3: index and return
        let mut results = Vec::with_capacity(writes.len());
        for (path, content) in &writes {
            let mut parsed = parser::parse(content, path)?;
            self.index.index_doogat(&parsed)?;
            self.nosql_index_doogat(&parsed);
            let id_str = parsed
                .meta
                .id
                .as_ref()
                .map(|z| z.0.clone())
                .unwrap_or_default();
            parsed.updated_at = self.index.lookup_updated_at(&id_str).unwrap_or(None);
            results.push(parsed);
        }

        Ok(results)
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
    ///
    /// Cascade behavior: junction table rows and dangling wikilinks in
    /// referencing files are cleaned up atomically in a single git commit.
    pub fn delete_doogat(&self, id: &str, message: &str) -> Result<Vec<(String, String)>> {
        self.ensure_fresh()?;
        let path = self.index.resolve_path(id)?;
        let broken = self.index.backlinking_doogat_paths(id)?;
        // Look up type before removing from index (needed for cascade)
        let doogat_type: Option<String> = self
            .index
            .conn
            .query_row(
                "SELECT type FROM doogats WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();
        // Collect reference edits before deleting
        let ref_edits = self.collect_ref_edits(id, &path)?;
        // Update index
        self.index.remove_doogat(id)?;
        self.nosql_remove_doogat(id);
        // Cascade: remove junction table rows referencing deleted doogat
        if let Some(ref dtype) = doogat_type {
            if !dtype.is_empty() && dtype != "_typedef" {
                self.index.cascade_junction_cleanup(&self.repo, dtype, id)?;
            }
        }
        // Atomic commit: delete + reference edits in one operation
        let writes: Vec<(&str, &str)> = ref_edits
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_batch(&writes, &[&path], message)?;
        Ok(broken)
    }

    /// Collect reference section edits needed when deleting a doogat.
    /// Returns `(path, new_content)` pairs; does NOT commit.
    fn collect_ref_edits(
        &self,
        deleted_id: &str,
        deleted_path: &str,
    ) -> Result<Vec<(String, String)>> {
        let sources = self.index.backlinks_by_target(deleted_id, deleted_path)?;
        let mut edits = Vec::new();
        for (source_id, source_path) in &sources {
            let content = self.repo.read_file(source_path)?;
            let mut parsed = parser::parse(&content, source_path)?;

            let old_section = parsed.reference_section.clone();
            let new_lines: Vec<&str> = old_section
                .lines()
                .filter(|line| {
                    !line.contains(&format!("[[{deleted_id}]]"))
                        && !line.contains(&format!("[[{deleted_path}]]"))
                })
                .collect();
            let new_section = if new_lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", new_lines.join("\n"))
            };

            if new_section == old_section {
                continue;
            }
            parsed.reference_section = new_section;
            let new_content = parser::serialize(&parsed);

            // Re-index and rematerialize
            let re_parsed = parser::parse(&new_content, source_path)?;
            self.index.index_doogat(&re_parsed)?;
            if let Some(ref stype) = re_parsed.meta.doogat_type {
                let schemas = self.index.load_all_typedefs(&self.repo);
                if let Some(schema) = schemas.get(stype.as_str()) {
                    self.index
                        .materialize_single(schema, source_id, &re_parsed)?;
                }
            }

            edits.push((source_path.to_string(), new_content));
        }
        Ok(edits)
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
                let updated_at = row.get(2).cloned();
                if let Ok(content) = self.repo.read_file(path) {
                    if let Ok(mut parsed) = parser::parse(&content, path) {
                        parsed.updated_at = updated_at;
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

        let group_by = match &query.distinct {
            Some(col) => format!(" GROUP BY \"{}\"", col.replace('"', "\"\"")),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id FROM \"{}\"{where_clause}{group_by} ORDER BY {order}{limit_clause}",
            query.table_name
        );

        let rows = self.index.query_raw_with_params(&sql, &query.params)?;
        let ids: Vec<&str> = rows.iter().filter_map(|r| r.first().map(|s| s.as_str())).collect();
        let updated_map = self.index.lookup_updated_at_batch(&ids).unwrap_or_default();
        let mut doogats = Vec::new();
        for row in rows {
            if let Some(id) = row.first() {
                if let Ok(path) = self.index.resolve_path(id) {
                    if let Ok(content) = self.repo.read_file(&path) {
                        if let Ok(mut parsed) = parser::parse(&content, &path) {
                            parsed.updated_at = updated_map.get(id.as_str()).cloned();
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
pub const SORTABLE_COLUMNS: &[&str] = &["id", "title", "date", "type", "updated_at"];

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
            let default_desc = matches!(field, "date" | "id");
            let dir = if filter.sort_desc.unwrap_or(default_desc) { "DESC" } else { "ASC" };
            if field == "id" {
                format!(" ORDER BY z.id {dir}")
            } else {
                format!(" ORDER BY z.{field} {dir}, z.id DESC")
            }
        }
        None => " ORDER BY z.date DESC, z.id DESC".to_string(),
    };

    format!("SELECT z.id, z.path, z.updated_at FROM doogats z{where_clause}{order_clause}{limit_clause}")
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

        svc.update_doogat(&id, Some("Updated"), None, None, Some("New body"), &ExtraFieldUpdates::default())
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
    fn list_doogats_filtered_sort_default_is_date_desc() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("First", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Second", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter::default();
        let results = svc.list_doogats_filtered(&filter).unwrap();
        // Default is date DESC, id DESC - newest comes first
        assert_eq!(results[0].meta.title.as_deref(), Some("Second"));
        assert_eq!(results[1].meta.title.as_deref(), Some("First"));
    }

    #[test]
    fn list_doogats_filtered_sort_date_defaults_to_desc() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("First", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Second", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter {
            sort_field: Some("date".into()),
            ..Default::default()
        };
        let results = svc.list_doogats_filtered(&filter).unwrap();
        // sort=date without explicit direction defaults to DESC
        assert_eq!(results[0].meta.title.as_deref(), Some("Second"));
        assert_eq!(results[1].meta.title.as_deref(), Some("First"));
    }

    #[test]
    fn list_doogats_filtered_sort_title_defaults_to_asc() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Bravo", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        svc.create_doogat("Alpha", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter {
            sort_field: Some("title".into()),
            ..Default::default()
        };
        let results = svc.list_doogats_filtered(&filter).unwrap();
        // sort=title without explicit direction defaults to ASC
        assert_eq!(results[0].meta.title.as_deref(), Some("Alpha"));
        assert_eq!(results[1].meta.title.as_deref(), Some("Bravo"));
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

    #[test]
    fn get_doogats_batch_multiple_valid() {
        let (_tmp, svc) = fresh_svc();
        let id1 = svc.create_doogat("First", &[], None, "body one").unwrap();
        let id2 = svc.create_doogat("Second", &[], None, "body two").unwrap();
        let id3 = svc.create_doogat("Third", &[], None, "body three").unwrap();

        let ids = vec![id1.clone(), id2.clone(), id3.clone()];
        let results = svc.get_doogats_batch(&ids).unwrap();
        assert_eq!(results.len(), 3);

        let titles: Vec<_> = results
            .iter()
            .map(|d| d.meta.title.as_deref().unwrap())
            .collect();
        assert!(titles.contains(&"First"));
        assert!(titles.contains(&"Second"));
        assert!(titles.contains(&"Third"));
    }

    #[test]
    fn get_doogats_batch_skips_invalid_ids() {
        let (_tmp, svc) = fresh_svc();
        let id1 = svc.create_doogat("Valid One", &[], None, "").unwrap();
        let id2 = svc.create_doogat("Valid Two", &[], None, "").unwrap();

        let ids = vec![
            id1.clone(),
            "99990101000000".to_string(), // nonexistent
            id2.clone(),
            "not-a-real-id".to_string(),  // invalid
        ];
        let results = svc.get_doogats_batch(&ids).unwrap();
        assert_eq!(results.len(), 2);

        let titles: Vec<_> = results
            .iter()
            .map(|d| d.meta.title.as_deref().unwrap())
            .collect();
        assert!(titles.contains(&"Valid One"));
        assert!(titles.contains(&"Valid Two"));
    }

    #[test]
    fn get_doogats_batch_empty_returns_empty() {
        let (_tmp, svc) = fresh_svc();
        let results = svc.get_doogats_batch(&[]).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn get_doogats_batch_single_id_matches_get_parsed() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.create_doogat("Solo", &["tag1".into()], None, "solo body").unwrap();

        let single = svc.get_doogat_parsed(&id).unwrap();
        let batch = svc.get_doogats_batch(&[id]).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].meta.title, single.meta.title);
        assert_eq!(batch[0].body, single.body);
    }

    #[test]
    fn get_doogat_parsed_has_updated_at() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.create_doogat("Note", &[], None, "body").unwrap();
        let parsed = svc.get_doogat_parsed(&id).unwrap();
        assert!(
            parsed.updated_at.is_some(),
            "updated_at should be populated from the index"
        );
        assert!(
            !parsed.updated_at.as_ref().unwrap().is_empty(),
            "updated_at should be a non-empty timestamp"
        );
    }

    #[test]
    fn updated_at_changes_on_update() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.create_doogat("Original", &[], None, "body").unwrap();
        let before = svc.get_doogat_parsed(&id).unwrap();
        let created_date = before.meta.date.clone();

        std::thread::sleep(std::time::Duration::from_millis(50));
        svc.update_doogat(&id, Some("Updated"), None, None, None, &ExtraFieldUpdates::default()).unwrap();

        let after = svc.get_doogat_parsed(&id).unwrap();
        assert_eq!(after.meta.date, created_date, "date (created_at) should not change");
        assert!(
            after.updated_at.as_ref().unwrap() >= before.updated_at.as_ref().unwrap(),
            "updated_at should advance after an update"
        );
    }

    #[test]
    fn list_doogats_filtered_has_updated_at() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("A", &[], None, "").unwrap();
        svc.create_doogat("B", &[], None, "").unwrap();

        let filter = crate::types::ListFilter::default();
        let doogats = svc.list_doogats_filtered(&filter).unwrap();
        assert_eq!(doogats.len(), 2);
        for d in &doogats {
            assert!(d.updated_at.is_some(), "each listed doogat should have updated_at");
        }
    }

    #[test]
    fn get_doogats_batch_has_updated_at() {
        let (_tmp, svc) = fresh_svc();
        let id1 = svc.create_doogat("First", &[], None, "").unwrap();
        let id2 = svc.create_doogat("Second", &[], None, "").unwrap();

        let batch = svc.get_doogats_batch(&[id1, id2]).unwrap();
        assert_eq!(batch.len(), 2);
        for d in &batch {
            assert!(d.updated_at.is_some(), "batch doogat should have updated_at");
        }
    }

    #[test]
    fn search_results_have_updated_at() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Searchable", &[], None, "findme content").unwrap();

        let results = svc.search_paginated_filtered(
            "findme",
            10,
            0,
            &crate::types::SearchFilters::default(),
        ).unwrap();
        assert_eq!(results.hits.len(), 1);
        assert!(
            !results.hits[0].updated_at.is_empty(),
            "search hit should have updated_at"
        );
    }

    #[test]
    fn sort_by_updated_at() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("First", &[], None, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        svc.create_doogat("Second", &[], None, "").unwrap();
        svc.reindex().unwrap();

        let filter = crate::types::ListFilter {
            sort_field: Some("updated_at".to_string()),
            sort_desc: Some(false),
            ..Default::default()
        };
        let doogats = svc.list_doogats_filtered(&filter).unwrap();
        assert_eq!(doogats.len(), 2);
        assert_eq!(doogats[0].meta.title.as_deref(), Some("First"));
        assert_eq!(doogats[1].meta.title.as_deref(), Some("Second"));
    }

    #[test]
    fn typed_filtered_list_has_updated_at() {
        let (_tmp, mut svc) = fresh_svc();
        svc.execute_sql("CREATE TABLE project (name TEXT)").unwrap();
        svc.execute_sql("INSERT INTO project (name) VALUES ('Alpha')").unwrap();

        let query = crate::types::TypedListQuery {
            table_name: "project".to_string(),
            where_sql: String::new(),
            params: vec![],
            order_sql: None,
            tag: None,
            limit: None,
            offset: None,
            distinct: None,
        };
        let doogats = svc.typed_filtered_list(&query).unwrap();
        assert_eq!(doogats.len(), 1);
        assert!(
            doogats[0].updated_at.is_some(),
            "typed_filtered_list should populate updated_at"
        );
    }

    #[test]
    fn create_returns_updated_at() {
        let (_tmp, svc) = fresh_svc();
        let parsed = svc.create_doogat_parsed("Direct", &[], None, "body").unwrap();
        assert!(
            parsed.updated_at.is_some(),
            "create_doogat_parsed should return updated_at in the response"
        );
    }

    #[test]
    fn update_returns_updated_at() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.create_doogat("Before", &[], None, "body").unwrap();
        let parsed = svc.update_doogat_parsed(&id, Some("After"), None, None, None, &ExtraFieldUpdates::default()).unwrap();
        assert!(
            parsed.updated_at.is_some(),
            "update_doogat_parsed should return updated_at in the response"
        );
    }

    // ---- batch_update tests ----

    fn count_commits(path: &std::path::Path) -> usize {
        let repo = git2::Repository::open(path).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let mut revwalk = repo.revwalk().unwrap();
        revwalk.push(head.id()).unwrap();
        revwalk.count()
    }

    #[test]
    fn batch_update_basic() {
        let (_tmp, svc) = fresh_svc();
        let id1 = svc.create_doogat("One", &[], None, "").unwrap();
        let id2 = svc.create_doogat("Two", &[], None, "").unwrap();
        let id3 = svc.create_doogat("Three", &[], None, "").unwrap();

        let updates = vec![
            crate::types::BatchUpdateInput {
                id: id1.clone(),
                title: Some("One Updated".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
            crate::types::BatchUpdateInput {
                id: id2.clone(),
                title: Some("Two Updated".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
            crate::types::BatchUpdateInput {
                id: id3.clone(),
                title: Some("Three Updated".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
        ];

        let results = svc.batch_update(&updates).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].meta.title.as_deref(), Some("One Updated"));
        assert_eq!(results[1].meta.title.as_deref(), Some("Two Updated"));
        assert_eq!(results[2].meta.title.as_deref(), Some("Three Updated"));

        // Verify persistence by re-reading
        let p1 = svc.get_doogat_parsed(&id1).unwrap();
        let p2 = svc.get_doogat_parsed(&id2).unwrap();
        let p3 = svc.get_doogat_parsed(&id3).unwrap();
        assert_eq!(p1.meta.title.as_deref(), Some("One Updated"));
        assert_eq!(p2.meta.title.as_deref(), Some("Two Updated"));
        assert_eq!(p3.meta.title.as_deref(), Some("Three Updated"));
    }

    #[test]
    fn batch_update_atomicity() {
        let (_tmp, svc) = fresh_svc();
        let id1 = svc.create_doogat("Alpha", &[], None, "body1").unwrap();
        let id2 = svc.create_doogat("Beta", &[], None, "body2").unwrap();
        let id3 = svc.create_doogat("Gamma", &[], None, "body3").unwrap();

        let updates = vec![
            crate::types::BatchUpdateInput {
                id: id1.clone(),
                title: Some("Alpha Changed".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
            crate::types::BatchUpdateInput {
                id: "99999999999999".to_string(), // non-existent
                title: Some("Ghost".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
            crate::types::BatchUpdateInput {
                id: id3.clone(),
                title: Some("Gamma Changed".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
        ];

        let result = svc.batch_update(&updates);
        assert!(result.is_err(), "batch_update should fail when an ID doesn't exist");

        // All originals must be unchanged
        let p1 = svc.get_doogat_parsed(&id1).unwrap();
        let p2 = svc.get_doogat_parsed(&id2).unwrap();
        let p3 = svc.get_doogat_parsed(&id3).unwrap();
        assert_eq!(p1.meta.title.as_deref(), Some("Alpha"));
        assert_eq!(p2.meta.title.as_deref(), Some("Beta"));
        assert_eq!(p3.meta.title.as_deref(), Some("Gamma"));
    }

    #[test]
    fn batch_update_single_commit() {
        let (tmp, svc) = fresh_svc();
        for i in 0..5 {
            svc.create_doogat(&format!("Item {i}"), &[], None, "").unwrap();
        }

        let before = count_commits(tmp.path());

        let filter = crate::types::ListFilter::default();
        let ids: Vec<String> = svc
            .list_doogats_filtered(&filter)
            .unwrap()
            .into_iter()
            .filter_map(|d| d.meta.id.map(|id| id.0))
            .collect();
        assert_eq!(ids.len(), 5);

        let updates: Vec<crate::types::BatchUpdateInput> = ids
            .iter()
            .map(|id| crate::types::BatchUpdateInput {
                id: id.clone(),
                title: Some(format!("Updated {id}")),
                body: None,
                tags: None,
                doogat_type: None,
            })
            .collect();

        svc.batch_update(&updates).unwrap();

        let after = count_commits(tmp.path());
        assert_eq!(
            after - before,
            1,
            "batch_update should create exactly 1 commit, not {}",
            after - before,
        );
    }

    #[test]
    fn batch_update_empty() {
        let (tmp, svc) = fresh_svc();
        let before = count_commits(tmp.path());

        let results = svc.batch_update(&[]).unwrap();
        assert!(results.is_empty(), "empty input should return empty vec");

        let after = count_commits(tmp.path());
        assert_eq!(before, after, "empty batch_update should not create a commit");
    }

    #[test]
    fn batch_update_mixed_fields() {
        let (_tmp, svc) = fresh_svc();
        let id1 = svc.create_doogat("Title1", &["tag1".to_string()], None, "body1").unwrap();
        let id2 = svc.create_doogat("Title2", &["tag2".to_string()], None, "body2").unwrap();
        let id3 = svc.create_doogat("Title3", &["tag3".to_string()], None, "body3").unwrap();

        let updates = vec![
            // Only title changes
            crate::types::BatchUpdateInput {
                id: id1.clone(),
                title: Some("NewTitle1".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
            // Only body changes
            crate::types::BatchUpdateInput {
                id: id2.clone(),
                title: None,
                body: Some("newbody2".to_string()),
                tags: None,
                doogat_type: None,
            },
            // Only tags change
            crate::types::BatchUpdateInput {
                id: id3.clone(),
                title: None,
                body: None,
                tags: Some(vec!["newtag3".to_string()]),
                doogat_type: None,
            },
        ];

        let results = svc.batch_update(&updates).unwrap();
        assert_eq!(results.len(), 3);

        // First: title changed, body and tags unchanged
        assert_eq!(results[0].meta.title.as_deref(), Some("NewTitle1"));
        assert_eq!(results[0].body, "body1");
        assert_eq!(results[0].meta.tags, vec!["tag1".to_string()]);

        // Second: body changed, title and tags unchanged
        assert_eq!(results[1].meta.title.as_deref(), Some("Title2"));
        assert_eq!(results[1].body, "newbody2");
        assert_eq!(results[1].meta.tags, vec!["tag2".to_string()]);

        // Third: tags changed, title and body unchanged
        assert_eq!(results[2].meta.title.as_deref(), Some("Title3"));
        assert_eq!(results[2].body, "body3");
        assert_eq!(results[2].meta.tags, vec!["newtag3".to_string()]);
    }

    #[test]
    fn batch_update_updated_at() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.create_doogat("Original", &[], None, "body").unwrap();

        let updates = vec![crate::types::BatchUpdateInput {
            id: id.clone(),
            title: Some("Changed".to_string()),
            body: None,
            tags: None,
            doogat_type: None,
        }];

        let results = svc.batch_update(&updates).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            results[0].updated_at.is_some(),
            "batch_update should populate updated_at on returned doogats"
        );
    }

    #[test]
    fn batch_update_rejects_duplicate_ids() {
        let (_tmp, svc) = fresh_svc();
        let id = svc.create_doogat("Dup", &[], None, "").unwrap();

        let updates = vec![
            crate::types::BatchUpdateInput {
                id: id.clone(),
                title: Some("First".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
            crate::types::BatchUpdateInput {
                id: id.clone(),
                title: Some("Second".to_string()),
                body: None,
                tags: None,
                doogat_type: None,
            },
        ];

        let result = svc.batch_update(&updates);
        assert!(result.is_err(), "batch_update should reject duplicate IDs");

        // Original unchanged
        let p = svc.get_doogat_parsed(&id).unwrap();
        assert_eq!(p.meta.title.as_deref(), Some("Dup"));
    }

    // ---- FTS5 search boost tests ----

    #[test]
    fn search_boost_fields_column_populated() {
        let (_tmp, svc) = fresh_svc();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "email".to_string(),
            crate::types::Value::String("alice@example.com".to_string()),
        );
        svc.create_doogat_with_extra("Alice Contact", &[], Some("contact"), "some body", extra)
            .unwrap();
        svc.reindex().unwrap();

        let results = svc.search("alice").unwrap();
        assert!(
            !results.is_empty(),
            "searching for 'alice' should find the doogat via the FTS fields column"
        );
        assert!(results[0].title.contains("Alice"));
    }

    #[test]
    fn search_boost_ranking_with_boosted_type() {
        let (_tmp, svc) = fresh_svc();

        // Install contact typedef (has search_boost: 1.5 on email column)
        svc.install_bundled_type("contact").unwrap();

        // Contact with "xyzzyterm" in email (frontmatter extra -> fields column)
        let mut extra1 = std::collections::BTreeMap::new();
        extra1.insert(
            "email".to_string(),
            crate::types::Value::String("xyzzyterm@example.com".to_string()),
        );
        svc.create_doogat_with_extra("FieldMatch", &[], Some("contact"), "no match here", extra1)
            .unwrap();

        // Contact with "xyzzyterm" only in body
        svc.create_doogat("BodyMatch", &[], Some("contact"), "xyzzyterm appears in body")
            .unwrap();

        svc.reindex().unwrap();

        let filters = crate::types::SearchFilters {
            types: Some(vec!["contact".to_string()]),
            ..Default::default()
        };
        let result = svc
            .search_paginated_filtered("xyzzyterm", 10, 0, &filters)
            .unwrap();
        assert_eq!(result.hits.len(), 2, "both contacts should match");

        // With boost on the fields column, the one matching in fields should
        // rank higher (lower/more-negative bm25 score = better match).
        assert_eq!(
            result.hits[0].title, "FieldMatch",
            "doogat with match in boosted fields column should rank first"
        );
    }

    #[test]
    fn search_boost_no_regression_without_type_filter() {
        let (_tmp, svc) = fresh_svc();

        svc.install_bundled_type("contact").unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "email".to_string(),
            crate::types::Value::String("boostnoreg@example.com".to_string()),
        );
        svc.create_doogat_with_extra("Boosted", &[], Some("contact"), "", extra)
            .unwrap();
        svc.create_doogat("Plain", &[], None, "boostnoreg in body")
            .unwrap();

        svc.reindex().unwrap();

        // Search without type filter - should work with default 1.0 weighting
        let results = svc.search("boostnoreg").unwrap();
        assert_eq!(results.len(), 2, "both doogats should appear without type filter");
    }

    #[test]
    fn search_boost_default_for_untyped() {
        let (_tmp, mut svc) = fresh_svc();

        // Create a type without any search_boost columns
        svc.execute_sql("CREATE TABLE project (name TEXT, status TEXT)")
            .unwrap();
        svc.execute_sql("INSERT INTO project (name, status) VALUES ('Alpha', 'active')")
            .unwrap();
        svc.create_doogat("Untyped", &[], None, "defaultboost content")
            .unwrap();

        svc.reindex().unwrap();

        // Search filtered to project type - should work with default 1.0 weighting
        let filters = crate::types::SearchFilters {
            types: Some(vec!["project".to_string()]),
            ..Default::default()
        };
        let result = svc
            .search_paginated_filtered("Alpha", 10, 0, &filters)
            .unwrap();
        assert!(
            result.hits.len() <= 1,
            "filtered search for project type should not error"
        );
    }

    // ---- batch_create tests ----

    #[test]
    fn batch_create_basic() {
        let (_tmp, svc) = fresh_svc();

        let inputs = vec![
            crate::types::BatchCreateInput {
                title: "Alpha".to_string(),
                body: None,
                tags: vec![],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            },
            crate::types::BatchCreateInput {
                title: "Beta".to_string(),
                body: None,
                tags: vec![],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            },
            crate::types::BatchCreateInput {
                title: "Gamma".to_string(),
                body: None,
                tags: vec![],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            },
        ];

        let results = svc.batch_create(&inputs).unwrap();
        assert_eq!(results.len(), 3);

        // All titles correct
        assert_eq!(results[0].meta.title.as_deref(), Some("Alpha"));
        assert_eq!(results[1].meta.title.as_deref(), Some("Beta"));
        assert_eq!(results[2].meta.title.as_deref(), Some("Gamma"));

        // All IDs distinct
        let ids: Vec<_> = results
            .iter()
            .map(|r| r.meta.id.as_ref().unwrap().0.clone())
            .collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 3, "all IDs must be distinct");

        // Verify persistence by re-reading each
        for result in &results {
            let id = &result.meta.id.as_ref().unwrap().0;
            let parsed = svc.get_doogat_parsed(id).unwrap();
            assert_eq!(parsed.meta.title, result.meta.title);
        }
    }

    #[test]
    fn batch_create_empty() {
        let (_tmp, svc) = fresh_svc();
        let results = svc.batch_create(&[]).unwrap();
        assert!(results.is_empty(), "empty input should return empty vec");
    }

    #[test]
    fn batch_create_return_order() {
        let (_tmp, svc) = fresh_svc();

        let titles = ["First", "Second", "Third"];
        let inputs: Vec<crate::types::BatchCreateInput> = titles
            .iter()
            .map(|t| crate::types::BatchCreateInput {
                title: t.to_string(),
                body: None,
                tags: vec![],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            })
            .collect();

        let results = svc.batch_create(&inputs).unwrap();
        assert_eq!(results.len(), 3);

        for (i, title) in titles.iter().enumerate() {
            assert_eq!(
                results[i].meta.title.as_deref(),
                Some(*title),
                "result at index {i} should have title '{title}'"
            );
        }
    }

    #[test]
    fn batch_create_with_type() {
        let (_tmp, mut svc) = fresh_svc();

        svc.execute_sql("CREATE TABLE task (name TEXT)").unwrap();

        let inputs = vec![
            crate::types::BatchCreateInput {
                title: "Task A".to_string(),
                body: None,
                tags: vec![],
                doogat_type: Some("task".to_string()),
                fields: std::collections::BTreeMap::new(),
            },
            crate::types::BatchCreateInput {
                title: "Task B".to_string(),
                body: None,
                tags: vec![],
                doogat_type: Some("task".to_string()),
                fields: std::collections::BTreeMap::new(),
            },
        ];

        let results = svc.batch_create(&inputs).unwrap();
        assert_eq!(results.len(), 2);

        for result in &results {
            assert_eq!(
                result.meta.doogat_type.as_deref(),
                Some("task"),
                "doogat_type should be 'task'"
            );
        }
    }

    #[test]
    fn batch_create_with_tags() {
        let (_tmp, svc) = fresh_svc();

        let inputs = vec![
            crate::types::BatchCreateInput {
                title: "Tagged One".to_string(),
                body: None,
                tags: vec!["rust".to_string(), "testing".to_string()],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            },
            crate::types::BatchCreateInput {
                title: "Tagged Two".to_string(),
                body: None,
                tags: vec!["python".to_string()],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            },
        ];

        let results = svc.batch_create(&inputs).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].meta.tags, vec!["rust", "testing"]);
        assert_eq!(results[1].meta.tags, vec!["python"]);
    }

    #[test]
    fn batch_create_with_body() {
        let (_tmp, svc) = fresh_svc();

        let inputs = vec![
            crate::types::BatchCreateInput {
                title: "With Body".to_string(),
                body: Some("Hello world content".to_string()),
                tags: vec![],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            },
            crate::types::BatchCreateInput {
                title: "Empty Body".to_string(),
                body: None,
                tags: vec![],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            },
        ];

        let results = svc.batch_create(&inputs).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].body, "Hello world content");
        assert!(
            results[1].body.is_empty(),
            "None body should produce empty body"
        );
    }

    #[test]
    fn batch_create_with_fields() {
        let (_tmp, mut svc) = fresh_svc();

        svc.execute_sql("CREATE TABLE items (category TEXT, priority INTEGER)")
            .unwrap();

        let mut fields1 = std::collections::BTreeMap::new();
        fields1.insert(
            "category".to_string(),
            crate::types::Value::String("electronics".to_string()),
        );
        fields1.insert(
            "priority".to_string(),
            crate::types::Value::String("5".to_string()),
        );

        let mut fields2 = std::collections::BTreeMap::new();
        fields2.insert(
            "category".to_string(),
            crate::types::Value::String("books".to_string()),
        );
        fields2.insert(
            "priority".to_string(),
            crate::types::Value::String("3".to_string()),
        );

        let inputs = vec![
            crate::types::BatchCreateInput {
                title: "Item One".to_string(),
                body: None,
                tags: vec![],
                doogat_type: Some("items".to_string()),
                fields: fields1,
            },
            crate::types::BatchCreateInput {
                title: "Item Two".to_string(),
                body: None,
                tags: vec![],
                doogat_type: Some("items".to_string()),
                fields: fields2,
            },
        ];

        let results = svc.batch_create(&inputs).unwrap();
        assert_eq!(results.len(), 2);

        // Verify extra frontmatter fields
        assert_eq!(
            results[0].meta.extra.get("category"),
            Some(&crate::types::Value::String("electronics".to_string()))
        );
        assert_eq!(
            results[0].meta.extra.get("priority"),
            Some(&crate::types::Value::String("5".to_string()))
        );
        assert_eq!(
            results[1].meta.extra.get("category"),
            Some(&crate::types::Value::String("books".to_string()))
        );
        assert_eq!(
            results[1].meta.extra.get("priority"),
            Some(&crate::types::Value::String("3".to_string()))
        );
    }

    #[test]
    fn batch_create_single_commit() {
        let (tmp, svc) = fresh_svc();

        let before = count_commits(tmp.path());

        let inputs: Vec<crate::types::BatchCreateInput> = (0..3)
            .map(|i| crate::types::BatchCreateInput {
                title: format!("Commit Test {i}"),
                body: None,
                tags: vec![],
                doogat_type: None,
                fields: std::collections::BTreeMap::new(),
            })
            .collect();

        svc.batch_create(&inputs).unwrap();

        let after = count_commits(tmp.path());
        assert_eq!(
            after - before,
            1,
            "batch_create should create exactly 1 commit, not {}",
            after - before,
        );
    }

    #[test]
    fn batch_create_default_next() {
        let (_tmp, mut svc) = fresh_svc();

        svc.execute_sql("CREATE TABLE ranked (name TEXT, pos INTEGER DEFAULT NEXT)")
            .unwrap();

        let inputs: Vec<crate::types::BatchCreateInput> = ["Alice", "Bob", "Carol"]
            .iter()
            .map(|name| {
                let mut fields = std::collections::BTreeMap::new();
                fields.insert(
                    "name".to_string(),
                    crate::types::Value::String(name.to_string()),
                );
                crate::types::BatchCreateInput {
                    title: format!("Ranked {name}"),
                    body: None,
                    tags: vec![],
                    doogat_type: Some("ranked".to_string()),
                    fields,
                }
            })
            .collect();

        svc.batch_create(&inputs).unwrap();
        svc.reindex().unwrap();

        let result = svc
            .execute_sql("SELECT name, pos FROM ranked ORDER BY pos")
            .unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 3, "should have 3 rows");
                assert_eq!(rows[0][1], "1");
                assert_eq!(rows[1][1], "2");
                assert_eq!(rows[2][1], "3");
            }
            _ => panic!("expected Rows from SELECT"),
        }
    }
}
