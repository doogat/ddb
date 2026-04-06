use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::git_ops::{self, GitRepo};
use crate::indexer::Index;
use crate::parser;
use crate::sql_engine::{SqlEngine, SqlResult, TransactionBuffer};
use crate::sync_manager::SyncManager;
use crate::traits::GitBackend;
use crate::types::{
    AttachmentInfo, BatchCreateInput, BatchUpdateInput, BrokenSequence, CommitHash,
    CompactDryRunInfo, CompactOptions, CompactionReport, DoogatId, DoogatMeta, FixReport,
    LinkDensityEntry, ListFilter, MaintenanceReport, NodeConfig, OrphanDoogat,
    PaginatedSearchResult, ParsedDoogat, RebuildReport, RecentDoogat, RenameReport, SearchFilters,
    SearchResult, SequenceInfo, SequenceNode, StaleDoogat, Suggestion, SyncReport, TableSchema,
    TagEntry, TagQueryFilter, TypedListQuery, UnlinkedMention,
};

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
        let updated_map = self
            .index
            .lookup_updated_at_batch(&id_refs)
            .unwrap_or_default();
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

        // Load schemas once if we'll need them for validation or rematerialization
        let schemas = if parsed.meta.doogat_type.is_some() {
            Some(self.list_type_schemas()?)
        } else {
            None
        };

        // Validate fields against typedef schema if this is a typed doogat with field changes
        if !extra.set.is_empty() || !extra.unset.is_empty() {
            if let (Some(ref type_name), Some(ref schemas)) = (&parsed.meta.doogat_type, &schemas) {
                self.validate_fields_with_schemas(schemas, type_name, &parsed.meta.extra)?;
            }
        }

        let new_content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &new_content, &format!("update doogat {id}"))?;
        // Re-parse to capture updated inline fields/wikilinks
        let mut parsed = parser::parse(&new_content, &path)?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);
        // Rematerialize type table row if this is a typed doogat
        if let (Some(ref type_name), Some(ref schemas)) = (&parsed.meta.doogat_type, &schemas) {
            if let Some(schema) = schemas.iter().find(|s| s.table_name == *type_name) {
                let id_str = parsed.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
                self.index.materialize_single(schema, id_str, &parsed)?;
            }
        }
        // Sync stored HEAD to avoid spurious incremental_reindex on next call
        self.index.store_head(&self.repo.head_oid()?.0)?;
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

        // Load schemas once for validation and rematerialization across all items
        let schemas = self.list_type_schemas()?;

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
            if let Some(ref unset) = update.unset_fields {
                for key in unset {
                    parsed.meta.extra.remove(key);
                }
            }
            if let Some(ref set) = update.fields {
                for (key, value) in set {
                    parsed.meta.extra.insert(key.clone(), value.clone());
                }
            }

            // Validate fields against typedef schema if fields were modified
            let has_field_changes = update.fields.is_some() || update.unset_fields.is_some();
            if has_field_changes {
                if let Some(ref type_name) = parsed.meta.doogat_type {
                    self.validate_fields_with_schemas(&schemas, type_name, &parsed.meta.extra)?;
                }
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

        // Phase 3: re-parse, index, rematerialize, return
        let mut results = Vec::with_capacity(updates.len());
        for (i, (path, new_content)) in writes.iter().enumerate() {
            let mut parsed = parser::parse(new_content, path)?;
            self.index.index_doogat(&parsed)?;
            self.nosql_index_doogat(&parsed);
            // Rematerialize type table row if this is a typed doogat
            if let Some(ref type_name) = parsed.meta.doogat_type {
                if let Some(schema) = schemas.iter().find(|s| s.table_name == *type_name) {
                    let id_str = parsed.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
                    self.index.materialize_single(schema, id_str, &parsed)?;
                }
            }
            let id = &updates[i].id;
            parsed.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
            results.push(parsed);
        }

        // Sync stored HEAD to avoid spurious incremental_reindex on next call
        self.index.store_head(&self.repo.head_oid()?.0)?;

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

        // Pre-compute NEXT counters per (table, column) for bare NEXT
        let mut next_counters: std::collections::BTreeMap<(String, String), i64> =
            std::collections::BTreeMap::new();
        // Track NEXT(partition) counters per (table, column, partition_value)
        let mut partitioned_counters: std::collections::BTreeMap<(String, String, String), i64> =
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
                            } else if dv.starts_with("NEXT(") && dv.ends_with(')') {
                                let partition_col = &dv[5..dv.len() - 1];
                                let partition_val = input
                                    .fields
                                    .get(partition_col)
                                    .map(|v| match v {
                                        crate::types::Value::String(s) => s.clone(),
                                        crate::types::Value::Number(n) => n.to_string(),
                                        crate::types::Value::Bool(b) => {
                                            if *b {
                                                "1".to_string()
                                            } else {
                                                "0".to_string()
                                            }
                                        }
                                        crate::types::Value::List(l) => format!("{l:?}"),
                                        crate::types::Value::Map(m) => format!("{m:?}"),
                                    })
                                    .unwrap_or_default();
                                let key =
                                    (type_name.clone(), col.name.clone(), partition_val.clone());
                                if let std::collections::btree_map::Entry::Vacant(e) =
                                    partitioned_counters.entry(key)
                                {
                                    let sql = format!(
                                        "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\" WHERE \"{}\" = ?1",
                                        col.name, schema.table_name, partition_col
                                    );
                                    let max_val: i64 = self
                                        .index
                                        .query_raw_with_params(
                                            &sql,
                                            &[rusqlite::types::Value::Text(partition_val)],
                                        )?
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

        // Phase 1: prepare all writes.
        // Each result slot is either an existing doogat (conflict-Ignore) or
        // None (pending creation).  Writes track the result-slot index so we
        // can fill them in after the atomic commit.
        let mut results: Vec<Option<ParsedDoogat>> = Vec::with_capacity(inputs.len());
        let mut writes: Vec<(usize, String, String)> = Vec::with_capacity(inputs.len());
        'inputs: for input in inputs {
            // ── on_conflict pre-check against unique_together ──
            if let Some(ref type_name) = input.doogat_type {
                if let Some(schema) = schemas.iter().find(|s| s.table_name == *type_name) {
                    if let Some(ref unique_groups) = schema.unique_together {
                        for group in unique_groups {
                            let mut joins = Vec::with_capacity(group.len());
                            let mut param_vals: Vec<rusqlite::types::Value> =
                                Vec::with_capacity(group.len() * 2 + 1);
                            let mut all_present = true;
                            for (i, col_name) in group.iter().enumerate() {
                                if let Some(val) = input.fields.get(col_name) {
                                    let val_str = match val {
                                        crate::types::Value::String(s) => s.clone(),
                                        crate::types::Value::Number(n) => n.to_string(),
                                        crate::types::Value::Bool(b) => {
                                            if *b {
                                                "1".to_string()
                                            } else {
                                                "0".to_string()
                                            }
                                        }
                                        crate::types::Value::List(l) => format!("{l:?}"),
                                        crate::types::Value::Map(m) => format!("{m:?}"),
                                    };
                                    let alias = format!("f{}", i + 1);
                                    let key_idx = param_vals.len() + 1;
                                    param_vals.push(rusqlite::types::Value::Text(col_name.clone()));
                                    let val_idx = param_vals.len() + 1;
                                    param_vals.push(rusqlite::types::Value::Text(val_str));
                                    joins.push(format!(
                                        "JOIN _ddb_fields {alias} ON \
                                         {alias}.doogat_id = d.id AND \
                                         {alias}.key = ?{key_idx} AND \
                                         {alias}.value = ?{val_idx}"
                                    ));
                                } else {
                                    all_present = false;
                                    break;
                                }
                            }
                            if !all_present {
                                continue;
                            }
                            param_vals.push(rusqlite::types::Value::Text(type_name.clone()));
                            let sql = format!(
                                "SELECT d.id FROM doogats d {} WHERE d.type = ?{} LIMIT 1",
                                joins.join(" "),
                                param_vals.len()
                            );
                            let rows = self.index.query_raw_with_params(&sql, &param_vals)?;
                            if let Some(existing_id) = rows.first().and_then(|r| r.first()) {
                                match input.on_conflict {
                                    crate::types::ConflictAction::Ignore => {
                                        results.push(Some(self.get_doogat_parsed(existing_id)?));
                                        continue 'inputs;
                                    }
                                    crate::types::ConflictAction::Error => {
                                        return Err(DoogatError::Validation(format!(
                                            "duplicate unique constraint on \
                                             type '{}' for columns {:?}",
                                            type_name, group
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let slot = results.len();
            results.push(None); // placeholder for the new doogat

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
                                            crate::types::Value::Number(n) => n.to_string(),
                                            crate::types::Value::Bool(b) => {
                                                if *b {
                                                    "1".to_string()
                                                } else {
                                                    "0".to_string()
                                                }
                                            }
                                            crate::types::Value::List(l) => format!("{l:?}"),
                                            crate::types::Value::Map(m) => format!("{m:?}"),
                                        })
                                        .unwrap_or_default();
                                    let key = (type_name.clone(), col.name.clone(), partition_val);
                                    let counter = partitioned_counters.get_mut(&key).unwrap();
                                    *counter += 1;
                                    extra.insert(
                                        col.name.clone(),
                                        crate::types::Value::String(counter.to_string()),
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
                                let exists = self
                                    .index
                                    .query_raw_with_params(
                                        "SELECT COUNT(*) > 0 FROM doogats WHERE id = ?1",
                                        &[rusqlite::types::Value::Text(val_str.clone())],
                                    )?
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
            writes.push((slot, path, content));
        }

        // Phase 2: atomic commit (only if there are new writes)
        if !writes.is_empty() {
            let write_refs: Vec<(&str, &str)> = writes
                .iter()
                .map(|(_, p, c)| (p.as_str(), c.as_str()))
                .collect();
            self.repo.commit_batch(
                &write_refs,
                &[],
                &format!("batch create {} doogats", writes.len()),
            )?;
        }

        // Phase 3: index new writes and fill result slots
        for (slot, path, content) in &writes {
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
            results[*slot] = Some(parsed);
        }

        // Sync _ddb_meta.head with the new git HEAD so subsequent ensure_fresh()
        // calls don't mistakenly think the index is stale and trigger
        // unnecessary incremental_reindex work.
        if !writes.is_empty() {
            self.index.store_head(&self.repo.head_oid()?.0)?;
        }

        Ok(results.into_iter().flatten().collect())
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

    /// Convert a `Value` to its canonical string representation for comparison
    /// against allowed_values and FK IDs. List/Map variants return None because
    /// they are not comparable to scalar constraints.
    fn value_to_comparable_string(val: &crate::types::Value) -> Option<String> {
        match val {
            crate::types::Value::String(s) => Some(s.clone()),
            crate::types::Value::Number(n) => Some(n.to_string()),
            crate::types::Value::Bool(b) => {
                Some(if *b { "1".to_string() } else { "0".to_string() })
            }
            crate::types::Value::List(_) | crate::types::Value::Map(_) => None,
        }
    }

    /// Validate extra fields against a pre-loaded typedef schema list.
    /// Callers load schemas once per operation to avoid redundant queries.
    fn validate_fields_with_schemas(
        &self,
        schemas: &[TableSchema],
        type_name: &str,
        extra: &std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<()> {
        let schema = match schemas.iter().find(|s| s.table_name == type_name) {
            Some(s) => s,
            None => return Ok(()), // no schema = no validation
        };
        for col in &schema.columns {
            // Validate allowed_values
            if let Some(ref allowed) = col.allowed_values {
                if let Some(val) = extra.get(&col.name) {
                    let val_str = match Self::value_to_comparable_string(val) {
                        Some(s) => s,
                        None => continue, // structured values can't match scalar allowed_values
                    };
                    if !allowed.contains(&val_str) {
                        return Err(DoogatError::Validation(format!(
                            "field '{}' value '{}' not in allowed values: {:?}",
                            col.name, val_str, allowed
                        )));
                    }
                }
            }
            // Validate FK references (both existence and target type match)
            if let Some(ref ref_table) = col.references {
                if let Some(val) = extra.get(&col.name) {
                    let val_str = match Self::value_to_comparable_string(val) {
                        Some(s) => s,
                        None => continue, // structured values can't be FK IDs
                    };
                    let exists = self
                        .index
                        .query_raw_with_params(
                            "SELECT COUNT(*) > 0 FROM doogats WHERE id = ?1 AND type = ?2",
                            &[
                                rusqlite::types::Value::Text(val_str.clone()),
                                rusqlite::types::Value::Text(ref_table.clone()),
                            ],
                        )?
                        .first()
                        .and_then(|r| r.first())
                        .map(|v| v == "1")
                        .unwrap_or(false);
                    if !exists {
                        return Err(DoogatError::Validation(format!(
                            "field '{}' references non-existent {} doogat '{}'",
                            col.name, ref_table, val_str
                        )));
                    }
                }
            }
        }
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
        // Cascade: remove materialized type table row and junction table rows
        if let Some(ref dtype) = doogat_type {
            if !dtype.is_empty() && dtype != "_typedef" {
                // Remove from materialized type table (ignore error if table doesn't exist)
                let _ = self.index.conn.execute(
                    &format!("DELETE FROM \"{}\" WHERE id = ?1", dtype),
                    params![id],
                );
                self.index.cascade_junction_cleanup(&self.repo, dtype, id)?;
            }
        }
        // Atomic commit: delete + reference edits in one operation
        let writes: Vec<(&str, &str)> = ref_edits
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_batch(&writes, &[&path], message)?;
        // Sync stored HEAD to avoid spurious incremental_reindex on next call
        self.index.store_head(&self.repo.head_oid()?.0)?;
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

    /// Query individual tag-doogat associations with optional filters.
    pub fn query_tags(&self, filter: &TagQueryFilter) -> Result<Vec<TagEntry>> {
        self.ensure_fresh()?;
        self.index.query_tags(filter)
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
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.first().map(|s| s.as_str()))
            .collect();
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

    let order_clause = match filter
        .sort_field
        .as_deref()
        .filter(|f| SORTABLE_COLUMNS.contains(f))
    {
        Some(field) => {
            let default_desc = matches!(field, "date" | "id");
            let dir = if filter.sort_desc.unwrap_or(default_desc) {
                "DESC"
            } else {
                "ASC"
            };
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
mod tests;
