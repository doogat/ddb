use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::git_ops;
use crate::parser;
use crate::types::{
    BatchCreateInput, BatchUpdateInput, DoogatId, DoogatMeta, ParsedDoogat, TableSchema,
};

use crate::traits::GitBackend;

use super::validation::{BareNextCounters, PartitionedNextCounters};
use super::{DoogatService, ExtraFieldUpdates};

/// Names that the doogat pipeline reserves for itself. A typed `createDoogat`
/// may legally include these in `fields` without triggering UNKNOWN_FIELD —
/// the pipeline either owns them (`id`, `title`, `type`, `date`,
/// `created_at`, `updated_at`) or routes them to a separate index
/// (`tags`). Mirrors the SQL validator's RESERVED_COLUMNS set in
/// `ddb-core/src/sql_engine/dml.rs`. PRD 00129 §1.
const RESERVED_TYPED_COLUMNS: &[&str] = &[
    "id",
    "title",
    "type",
    "date",
    "created_at",
    "updated_at",
    "tags",
];

impl<G: GitBackend> DoogatService<G> {
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

        apply_field_updates(&mut parsed, title, tags, doogat_type, body, extra);

        let schemas = if parsed.meta.doogat_type.is_some() {
            Some(self.list_type_schemas()?)
        } else {
            None
        };

        let has_field_changes = !extra.set.is_empty() || !extra.unset.is_empty();
        if has_field_changes {
            if let (Some(ref type_name), Some(ref schemas)) = (&parsed.meta.doogat_type, &schemas) {
                self.validate_fields_with_schemas(schemas, type_name, &parsed.meta.extra)?;
            }
        }

        let new_content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &new_content, &format!("update doogat {id}"))?;
        let mut parsed = self.reindex_and_rematerialize(&new_content, &path, schemas.as_deref())?;
        // Sync stored HEAD to avoid spurious incremental_reindex on next call
        self.index.store_head(&self.repo.head_oid()?.0)?;
        parsed.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
        Ok(parsed)
    }

    /// Re-parse content, index, dual-write, and rematerialize the type table row.
    fn reindex_and_rematerialize(
        &self,
        content: &str,
        path: &str,
        schemas: Option<&[TableSchema]>,
    ) -> Result<ParsedDoogat> {
        let mut parsed = parser::parse(content, path)?;
        self.index.index_doogat(&parsed)?;
        self.nosql_index_doogat(&parsed);
        if let (Some(ref type_name), Some(schemas)) = (&parsed.meta.doogat_type, schemas) {
            if let Some(schema) = schemas.iter().find(|s| s.table_name == *type_name) {
                let id_str = parsed.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
                self.index.materialize_single(schema, id_str, &parsed)?;
            }
        }
        parsed.updated_at = parsed
            .meta
            .id
            .as_ref()
            .and_then(|z| self.index.lookup_updated_at(&z.0).unwrap_or(None));
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
        self.reject_duplicate_update_ids(updates)?;
        self.ensure_fresh()?;

        let schemas = self.list_type_schemas()?;

        // Phase 1: prepare all writes (fail-fast, no side effects)
        let mut writes: Vec<(String, String)> = Vec::with_capacity(updates.len());
        for update in updates {
            writes.push(self.prepare_update(update, &schemas)?);
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
        for (path, new_content) in &writes {
            let parsed = self.reindex_and_rematerialize(new_content, path, Some(&schemas))?;
            results.push(parsed);
        }

        // Sync stored HEAD to avoid spurious incremental_reindex on next call
        self.index.store_head(&self.repo.head_oid()?.0)?;

        Ok(results)
    }

    /// Reject batch updates that contain the same doogat ID more than once.
    fn reject_duplicate_update_ids(&self, updates: &[BatchUpdateInput]) -> Result<()> {
        let mut seen = std::collections::HashSet::with_capacity(updates.len());
        for u in updates {
            if !seen.insert(&u.id) {
                return Err(DoogatError::Validation(format!(
                    "duplicate id in batch: {}",
                    u.id
                )));
            }
        }
        Ok(())
    }

    /// Prepare a single update: read, merge fields, validate, serialize.
    /// Returns `(path, new_content)` without side effects.
    fn prepare_update(
        &self,
        update: &BatchUpdateInput,
        schemas: &[TableSchema],
    ) -> Result<(String, String)> {
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

        let has_field_changes = update.fields.is_some() || update.unset_fields.is_some();
        if has_field_changes {
            if let Some(ref type_name) = parsed.meta.doogat_type {
                self.validate_fields_with_schemas(schemas, type_name, &parsed.meta.extra)?;
            }
        }

        let new_content = parser::serialize(&parsed);
        Ok((path, new_content))
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

        let schemas = self.list_type_schemas()?;
        let (mut next_counters, mut partitioned_counters) =
            self.precompute_next_counters(inputs, &schemas)?;

        // Phase 1: prepare all writes.
        let mut results: Vec<Option<ParsedDoogat>> = Vec::with_capacity(inputs.len());
        let mut writes: Vec<(usize, String, String)> = Vec::with_capacity(inputs.len());
        for input in inputs {
            if let Some(existing) = self.check_unique_constraints(input, &schemas)? {
                results.push(Some(existing));
                continue;
            }

            let slot = results.len();
            results.push(None);

            let (path, content) = self.prepare_create(
                input,
                &schemas,
                &mut next_counters,
                &mut partitioned_counters,
            )?;
            writes.push((slot, path, content));
        }

        // Phase 2: atomic commit
        self.commit_batch_creates(&writes)?;

        // Phase 3: index new writes and fill result slots
        self.index_batch_creates(&writes, &schemas, &mut results)?;

        Ok(results.into_iter().flatten().collect())
    }

    /// Build a single doogat for batch_create: generate ID, resolve defaults, serialize.
    fn prepare_create(
        &self,
        input: &BatchCreateInput,
        schemas: &[TableSchema],
        next_counters: &mut BareNextCounters,
        partitioned_counters: &mut PartitionedNextCounters,
    ) -> Result<(String, String)> {
        // PRD 00129 §1: typed-create rejection happens before any defaults
        // are resolved or git is touched. Catches TYPE_NOT_REGISTERED and
        // UNKNOWN_FIELD (NOT_NULL_VIOLATION runs after default resolution
        // below since defaults can satisfy a NOT NULL column).
        Self::validate_typed_create_pre_defaults(input, schemas)?;

        let id = self.unique_id();
        let id_str = id.to_string();

        let folder = input
            .doogat_type
            .as_deref()
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let path = git_ops::doogat_path(&id_str, input.doogat_type.as_deref(), folder);

        let mut extra = input.fields.clone();
        self.resolve_column_defaults(input, schemas, &mut extra, next_counters, partitioned_counters)?;
        Self::validate_typed_create_post_defaults(input, schemas, &extra)?;
        self.validate_column_constraints(input, schemas, &extra)?;

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
        Ok((path, content))
    }

    /// Phase 2: atomic commit for batch creates (no-op when writes is empty).
    fn commit_batch_creates(&self, writes: &[(usize, String, String)]) -> Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let write_refs: Vec<(&str, &str)> = writes
            .iter()
            .map(|(_, p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_batch(
            &write_refs,
            &[],
            &format!("batch create {} doogats", writes.len()),
        )?;
        Ok(())
    }

    /// Phase 3: index new writes, fill result slots, sync HEAD.
    fn index_batch_creates(
        &self,
        writes: &[(usize, String, String)],
        schemas: &[TableSchema],
        results: &mut [Option<ParsedDoogat>],
    ) -> Result<()> {
        for (slot, path, content) in writes {
            // PRD 00129 §1: pass schemas so the materialized type-table row
            // is written via `materialize_single` after the git commit.
            // Before this, batch_create populated only the `doogats` index
            // row and the materialized typed table stayed empty —
            // duplicates couldn't violate UNIQUE because there was nothing
            // to violate.
            let parsed = self.reindex_and_rematerialize(content, path, Some(schemas))?;
            results[*slot] = Some(parsed);
        }
        if !writes.is_empty() {
            self.index.store_head(&self.repo.head_oid()?.0)?;
        }
        Ok(())
    }

    /// PRD 00129 §1: pre-defaults validation for typed creates. Runs
    /// before `resolve_column_defaults` so unknown fields and unregistered
    /// types reject without polluting the defaults pipeline. NOT NULL
    /// runs in [`validate_typed_create_post_defaults`] because a column
    /// default can legitimately satisfy a NOT NULL column.
    fn validate_typed_create_pre_defaults(
        input: &BatchCreateInput,
        schemas: &[TableSchema],
    ) -> Result<()> {
        let type_name = match input.doogat_type {
            Some(ref t) => t,
            None => return Ok(()),
        };
        let schema = schemas.iter().find(|s| s.table_name == *type_name);
        // The PRD requires an unregistered type to reject. Today the
        // singular `create_doogat_with_extra` (CLI / FFI) silently allows
        // it, so we only reject from the GraphQL surface — which routes
        // through batch_create. Untyped creates skip above; typed creates
        // with no fields supplied still reject so callers can't silently
        // tag a doogat with a nonexistent type via this path.
        let schema = match schema {
            Some(s) => s,
            None => {
                return Err(DoogatError::type_not_registered(type_name.clone()));
            }
        };

        // UNKNOWN_FIELD: every key in input.fields must either be a
        // declared column or one of the reserved core columns. Reserved
        // names are owned by the doogat pipeline and pass through silently
        // (matches the SQL validator's reserved-set behavior).
        for key in input.fields.keys() {
            if schema.columns.iter().any(|c| &c.name == key) {
                continue;
            }
            if RESERVED_TYPED_COLUMNS.contains(&key.as_str()) {
                continue;
            }
            return Err(DoogatError::unknown_field(type_name.clone(), key.clone()));
        }

        Ok(())
    }

    /// PRD 00129 §1: NOT NULL enforcement once column defaults have been
    /// resolved. A required column is satisfied when `extra` carries a
    /// non-null value for it, when the typedef declares a `default_value`
    /// (already merged into `extra` by `resolve_column_defaults`), or — for
    /// the special `title` column — when the typedef declares a
    /// `title_template` (the title is synthesized at INSERT time, mirroring
    /// the SQL validator's exemption from PRD 00122).
    fn validate_typed_create_post_defaults(
        input: &BatchCreateInput,
        schemas: &[TableSchema],
        extra: &std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<()> {
        let type_name = match input.doogat_type {
            Some(ref t) => t,
            None => return Ok(()),
        };
        let schema = match schemas.iter().find(|s| s.table_name == *type_name) {
            Some(s) => s,
            None => return Ok(()), // pre-defaults check already rejected
        };

        for col in &schema.columns {
            if !col.required {
                continue;
            }
            // Title is satisfied by `input.title` always (BatchCreateInput
            // requires it as a non-Option String) or by the title_template
            // synthesis path. Skip the NOT NULL check for it.
            if col.name == "title" {
                continue;
            }
            if col.default_value.is_some() {
                continue;
            }
            if extra.contains_key(&col.name) {
                continue;
            }
            return Err(DoogatError::not_null_violation(
                type_name.clone(),
                col.name.clone(),
            ));
        }

        Ok(())
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
        // RESTRICT: reject the delete if any typed-table row holds `id` in a
        // `NOT NULL REFERENCES` column. Wikilink stripping would otherwise
        // leave that row with NULL in a NOT NULL column (#10).
        self.index.check_restrict_blocks_delete(&self.repo, id)?;
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
}

fn apply_field_updates(
    parsed: &mut ParsedDoogat,
    title: Option<&str>,
    tags: Option<&[String]>,
    doogat_type: Option<&str>,
    body: Option<&str>,
    extra: &ExtraFieldUpdates<'_>,
) {
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
}
