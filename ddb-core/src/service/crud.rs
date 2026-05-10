use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::git_ops;
use crate::parser;
use crate::sql_engine::typed_insert::{prepare_typed_insert_validate, TypedInsertCounters};
use crate::sql_engine::{apply_updates_to_doogat, build_data_doogat};
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

/// Stringify a `Value` for purposes of intra-batch unique-tuple comparison.
/// Same encoding as `extra_to_template_col_values` — sticking with one
/// representation keeps "the unique-key check" and "the title template
/// substitution" agreeing on what counts as the same value.
fn value_to_unique_key(v: &crate::types::Value) -> String {
    use crate::types::Value;
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        _ => String::new(),
    }
}

/// Composite key for intra-batch unique-tuple tracking: type name +
/// the column tuple (as declared in `unique_together`) + the values
/// supplied in `input.fields` for those columns.
type UniqueKey = (String, Vec<String>, Vec<String>);

/// If `input` would collide on any of its typedef's `unique_together`
/// tuples with a row already prepared earlier in the same batch, return
/// the array index of that earlier input alongside the colliding key.
/// Used by `batch_create` to honour `on_conflict` semantics for intra-batch
/// duplicates (issue #12) and to surface a structured `UNIQUE_VIOLATION`
/// error (PRD 00131).
fn find_intra_batch_duplicate(
    input: &BatchCreateInput,
    schemas: &[TableSchema],
    seen: &std::collections::HashMap<UniqueKey, usize>,
) -> Option<(usize, UniqueKey)> {
    for key in input_unique_keys(input, schemas) {
        if let Some(prior_idx) = seen.get(&key) {
            return Some((*prior_idx, key));
        }
    }
    None
}

/// Record every unique-tuple key derivable from `input` so later inputs in
/// the same batch can detect collisions via `find_intra_batch_duplicate`.
fn record_intra_batch_unique(
    input: &BatchCreateInput,
    schemas: &[TableSchema],
    seen: &mut std::collections::HashMap<UniqueKey, usize>,
    input_idx: usize,
) {
    for key in input_unique_keys(input, schemas) {
        seen.entry(key).or_insert(input_idx);
    }
}

/// Yield one composite key per declared `unique_together` group on the
/// input's typedef, skipping groups that don't have all their columns
/// supplied in `input.fields` (those can't form a complete tuple yet, so
/// they can't collide).
fn input_unique_keys(input: &BatchCreateInput, schemas: &[TableSchema]) -> Vec<UniqueKey> {
    let type_name = match input.doogat_type.as_deref() {
        Some(t) => t,
        None => return vec![],
    };
    let schema = match schemas.iter().find(|s| s.table_name == type_name) {
        Some(s) => s,
        None => return vec![],
    };
    let groups = match schema.unique_together.as_ref() {
        Some(g) => g,
        None => return vec![],
    };
    groups
        .iter()
        .filter_map(|group| {
            let values: Option<Vec<String>> = group
                .iter()
                .map(|col| input.fields.get(col).map(value_to_unique_key))
                .collect();
            values.map(|vals| (type_name.to_string(), group.clone(), vals))
        })
        .collect()
}

/// PRD 00139 §4: returns the typedef's `table_name` when this input
/// targets a SINGLETON typedef registered in `schemas`. Returns `None`
/// for untyped inputs, unregistered types, or non-singleton typedefs.
fn singleton_type_for(input: &BatchCreateInput, schemas: &[TableSchema]) -> Option<String> {
    let type_name = input.doogat_type.as_deref()?;
    let schema = schemas.iter().find(|s| s.table_name == type_name)?;
    if schema.singleton {
        Some(type_name.to_string())
    } else {
        None
    }
}

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
    ///
    /// PRD 00133: when `doogat_type` resolves to a registered typedef, the
    /// extra fields route through `prepare_typed_insert_validate` +
    /// `build_data_doogat` so the CLI/FFI surface matches the SQL/GraphQL
    /// behavior — `REFERENCES` columns land in the reference zone,
    /// `allowed_values` and FK constraints are enforced, and unknown columns
    /// reject with `UNKNOWN_FIELD`. PRD 00129 §T3 still applies for
    /// unregistered types: untyped or unregistered-type creates keep the
    /// "straight frontmatter" behavior.
    pub fn create_doogat_with_extra(
        &self,
        title: &str,
        tags: &[String],
        doogat_type: Option<&str>,
        body: &str,
        extra: std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<ParsedDoogat> {
        // PRD 00136 / #16: align with sibling entry points (`update_doogat`,
        // `read_doogat`, `delete_doogat`, `search`, `rename_doogat`) so every
        // public service method that touches the index refreshes on entry.
        // The actor path opts out via `set_skip_stale_check(true)` already.
        self.ensure_fresh()?;
        let id = self.unique_id();
        let id_str = id.to_string();

        let folder = doogat_type
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let path = git_ops::doogat_path(&id_str, doogat_type, folder);

        let typedef = match doogat_type {
            Some(t) => self
                .list_type_schemas()?
                .into_iter()
                .find(|s| s.table_name == t),
            None => None,
        };

        // Lower the explicit args into a synthetic `BatchCreateInput` so the
        // typed-create pipeline (pre-defaults, helper, post-defaults) runs
        // with the same shape `batch_create` uses, no parallel logic.
        let synth_input = BatchCreateInput {
            title: Some(title.to_owned()),
            body: Some(body.to_owned()),
            tags: tags.to_vec(),
            doogat_type: doogat_type.map(str::to_owned),
            fields: extra.clone(),
            on_conflict: crate::types::ConflictAction::Error,
        };

        let mut parsed = match typedef.as_ref() {
            Some(schema) => self.build_typed_single_create(&synth_input, &id, &path, schema)?,
            None => ParsedDoogat {
                meta: DoogatMeta {
                    id: Some(id.clone()),
                    title: Some(title.to_owned()),
                    date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
                    doogat_type: doogat_type.map(str::to_owned),
                    tags: tags.to_vec(),
                    extra,
                },
                body: body.to_owned(),
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
                body_tags: vec![],
                checkboxes: vec![],
                path: path.clone(),
                updated_at: None,
            },
        };

        let content = parser::serialize(&parsed);
        self.repo
            .commit_file(&path, &content, &format!("create doogat {id_str}"))?;
        self.index.index_doogat(&parsed)?;
        // PRD 00134 blind-review C1: typed creates must populate the
        // typed-table row + auto-junctions atomically with `index_doogat`.
        // Without this, `<type>_<col>` junctions stay empty until the next
        // `ddb query` triggers an implicit `ensure_fresh` reindex — same
        // parity gap that `batch_create` closed via
        // `reindex_and_rematerialize` (see PRD 00129 §1).
        if let Some(schema) = typedef.as_ref() {
            self.index.materialize_single(schema, &id_str, &parsed)?;
        }
        self.nosql_index_doogat(&parsed);
        parsed.updated_at = self.index.lookup_updated_at(&id_str).unwrap_or(None);

        Ok(parsed)
    }

    /// Single-row typed-create branch of `create_doogat_with_extra`. Mirrors
    /// `build_typed_create` (used by `batch_create`) but with a fresh
    /// per-call `TypedInsertCounters` (no batch-aware NEXT(partition) state
    /// to share).
    fn build_typed_single_create(
        &self,
        input: &BatchCreateInput,
        id: &DoogatId,
        path: &str,
        schema: &TableSchema,
    ) -> Result<ParsedDoogat> {
        let schemas_slice = std::slice::from_ref(schema);
        Self::validate_typed_create_pre_defaults(input, schemas_slice)?;

        let mut col_values = stringify_typed_input_fields(input, schema)?;
        let mut counters = TypedInsertCounters::default();

        prepare_typed_insert_validate(schema, &mut col_values, &mut counters, &self.index.conn)?;

        Self::validate_typed_create_post_defaults(input, schema, &col_values)?;

        // CLI/FFI always supplies an explicit title; insert it into
        // col_values so `build_data_doogat`'s priority-1 title resolution
        // picks it up.
        if let Some(ref t) = input.title {
            col_values.insert("title".to_string(), t.clone());
        }

        let ref_folder_types: std::collections::HashSet<String> = schema
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref())
            .filter(|ref_table| self.index.type_uses_folder(ref_table, &self.repo))
            .cloned()
            .collect();

        let mut parsed = build_data_doogat(
            id,
            schema,
            &col_values,
            &ref_folder_types,
            Some(&self.index.conn),
        );
        parsed.meta.tags = input.tags.clone();
        if let Some(ref b) = input.body {
            if !b.is_empty() {
                parsed.body = b.clone();
            }
        }
        parsed.path = path.to_owned();
        Ok(parsed)
    }

    /// Create a doogat from raw Markdown content (for FFI consumers).
    ///
    /// Parses the content to extract/generate an ID, determines storage path,
    /// commits, indexes, and dual-writes. Returns the doogat ID.
    pub fn create_doogat_raw(&self, content: &str, message: &str) -> Result<String> {
        self.ensure_fresh()?;
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
        let schemas = self.list_type_schemas()?;
        let content = if let Some(type_name) = parsed.meta.doogat_type.as_deref() {
            let input = build_batch_create_from_parsed(&parsed);
            Self::validate_typed_create_pre_defaults(&input, &schemas)?;
            let schema = schemas
                .iter()
                .find(|s| s.table_name == type_name)
                .ok_or_else(|| DoogatError::type_not_registered(type_name.to_string()))?;
            let _ = self.check_singleton_constraint(&input, &schemas)?;
            let _ = self.check_unique_constraints(&input, &schemas)?;
            let normalized = self.build_typed_single_create(
                &input,
                &DoogatId(id.clone()),
                &rel_path,
                schema,
            )?;
            parser::serialize(&normalized)
        } else {
            content.to_string()
        };

        self.repo.commit_file(&rel_path, &content, message)?;
        let parsed = parser::parse(&content, &rel_path)?;
        self.index.index_doogat(&parsed)?;
        // PRD 00134 blind-review I2: same atomic-junction parity as
        // `create_doogat_with_extra` — when raw Markdown carries a
        // registered typedef, populate the typed table + auto-junctions
        // alongside the metadata index update.
        if let Some(type_name) = parsed.meta.doogat_type.as_deref() {
            if let Some(schema) = self
                .list_type_schemas()?
                .into_iter()
                .find(|s| s.table_name == type_name)
            {
                self.index.materialize_single(&schema, &id, &parsed)?;
            }
        }
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

        // Resolve schemas BEFORE applying field updates so typed-column SETs
        // route to the correct zone (REFERENCES → reference section, etc.).
        // PRD 00134 cycle-1 review C1 task #2.
        let schemas = if parsed.meta.doogat_type.is_some() {
            Some(self.list_type_schemas()?)
        } else {
            None
        };
        let schema = schemas.as_ref().and_then(|all| {
            parsed
                .meta
                .doogat_type
                .as_deref()
                .and_then(|t| all.iter().find(|s| s.table_name == t))
        });

        // Validate the user-supplied SET fields BEFORE routing them. After
        // routing, REFERENCES values land in the reference zone, hidden
        // from the `parsed.meta.extra`-based validator. We validate against
        // the input map directly so FK/allowed_values rejections still
        // fire on the typed UPDATE path.
        let has_field_changes = !extra.set.is_empty() || !extra.unset.is_empty();
        if has_field_changes {
            if let (Some(ref type_name), Some(ref schemas)) = (&parsed.meta.doogat_type, &schemas) {
                self.validate_fields_with_schemas(schemas, type_name, extra.set)?;
            }
        }

        apply_field_updates(&mut parsed, title, tags, doogat_type, body, extra, schema);

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
        let message = format!("batch update {} doogats", updates.len());
        self.batch_update_with_message(updates, &message)
    }

    pub(crate) fn batch_update_with_message(
        &self,
        updates: &[BatchUpdateInput],
        message: &str,
    ) -> Result<Vec<ParsedDoogat>> {
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
            message,
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
    ///
    /// Routes typed REFERENCES-column SETs to the reference zone via the
    /// shared `apply_field_updates` helper so the auto-junction stays in
    /// sync after a `batch_update` mutation. PRD 00134 cycle-1 review C1
    /// task #2.
    fn prepare_update(
        &self,
        update: &BatchUpdateInput,
        schemas: &[TableSchema],
    ) -> Result<(String, String)> {
        let path = self.index.resolve_path(&update.id)?;
        let content = self.repo.read_file(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        let final_type = update
            .doogat_type
            .as_deref()
            .or(parsed.meta.doogat_type.as_deref());
        if let Some(final_type) = final_type {
            if !schemas.iter().any(|s| s.table_name == final_type) {
                return Err(DoogatError::type_not_registered(final_type.to_string()));
            }
            self.check_singleton_update_constraint(&update.id, final_type, schemas)?;
        }

        let schema = parsed
            .meta
            .doogat_type
            .as_deref()
            .or(update.doogat_type.as_deref())
            .and_then(|t| schemas.iter().find(|s| s.table_name == t));

        let empty_set: std::collections::BTreeMap<String, crate::types::Value> =
            std::collections::BTreeMap::new();
        let empty_unset: Vec<String> = Vec::new();
        let extra = ExtraFieldUpdates {
            set: update.fields.as_ref().unwrap_or(&empty_set),
            unset: update.unset_fields.as_deref().unwrap_or(&empty_unset),
        };

        // Validate against the input SET map BEFORE routing. See
        // `update_doogat_parsed` for rationale (REFERENCES values move to
        // the reference zone, hiding them from the meta.extra validator).
        let has_field_changes = update.fields.is_some() || update.unset_fields.is_some();
        if has_field_changes {
            if let Some(ref type_name) = parsed.meta.doogat_type {
                self.validate_fields_with_schemas(schemas, type_name, extra.set)?;
            }
        }

        apply_field_updates(
            &mut parsed,
            update.title.as_deref(),
            update.tags.as_deref(),
            update.doogat_type.as_deref(),
            update.body.as_deref(),
            &extra,
            schema,
        );

        let new_content = parser::serialize(&parsed);
        Ok((path, new_content))
    }

    /// Batch-create multiple doogats in a single atomic commit.
    ///
    /// Generates unique IDs, resolves typedef defaults (including DEFAULT NEXT),
    /// validates constraints, and commits all files atomically.
    ///
    /// Issue #12: cross-batch duplicates (DB conflict) and intra-batch
    /// duplicates (two inputs in the same batch with the same unique
    /// tuple) both honour `on_conflict`. For `Ignore`, the response payload
    /// at the duplicate input's array index is the surviving row's
    /// payload — the rejected ID is discarded. For `Error`, the whole
    /// batch fails.
    pub fn batch_create(&self, inputs: &[BatchCreateInput]) -> Result<Vec<ParsedDoogat>> {
        let message = format!("batch create {} doogats", inputs.len());
        self.batch_create_with_message(inputs, &message)
    }

    pub(crate) fn batch_create_with_message(
        &self,
        inputs: &[BatchCreateInput],
        message: &str,
    ) -> Result<Vec<ParsedDoogat>> {
        if inputs.is_empty() {
            return Ok(vec![]);
        }

        self.ensure_fresh()?;

        let schemas = self.list_type_schemas()?;
        // Cross-input NEXT counters. Start empty; the helper's
        // `prepare_typed_insert_validate` lazy-seeds each entry from
        // `MAX(col)` on first encounter and increments per row, so the
        // batch-aware sequence stays monotonic across inputs sharing a
        // table without an upfront pre-seed pass.
        let mut next_counters = BareNextCounters::new();
        let mut partitioned_counters = PartitionedNextCounters::new();

        // Phase 1: prepare all writes.
        // `intra_dup_links[i] = Some(j)` means input i is an intra-batch
        // duplicate of input j; resolve `results[i] = results[j]` after
        // Phase 3 fills the surviving row.
        let mut results: Vec<Option<ParsedDoogat>> = (0..inputs.len()).map(|_| None).collect();
        let mut writes: Vec<(usize, String, String)> = Vec::with_capacity(inputs.len());
        let mut intra_dup_links: Vec<Option<usize>> = vec![None; inputs.len()];
        let mut seen_unique: std::collections::HashMap<UniqueKey, usize> =
            std::collections::HashMap::new();
        // PRD 00139 §4: track singleton inserts within the same batch so a
        // second INSERT into the same SINGLETON typedef is rejected before
        // any commit lands. Maps `type_name -> first surviving input idx`.
        let mut seen_singleton: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (input_idx, input) in inputs.iter().enumerate() {
            // PRD 00139 §3 layer 1: pre-check the SINGLETON constraint
            // before the UNIQUE check so callers see SINGLETON_VIOLATION
            // (the stronger constraint) when both apply.
            if let Some(existing) = self.check_singleton_constraint(input, &schemas)? {
                results[input_idx] = Some(existing);
                continue;
            }

            if let Some(existing) = self.check_unique_constraints(input, &schemas)? {
                results[input_idx] = Some(existing);
                continue;
            }

            // PRD 00139 §4: intra-batch SINGLETON tracker. A second
            // SINGLETON insert into the same typedef in this batch must
            // reject (Error) or skip-to-survivor (Ignore) before commit.
            if let Some(type_name) = singleton_type_for(input, &schemas) {
                if let Some(&prior_idx) = seen_singleton.get(&type_name) {
                    match input.on_conflict {
                        crate::types::ConflictAction::Ignore => {
                            intra_dup_links[input_idx] = Some(prior_idx);
                            continue;
                        }
                        crate::types::ConflictAction::Error => {
                            // The prior row's id isn't materialized yet
                            // (we're pre-commit), so use the placeholder
                            // marker `<intra-batch>` to signal the
                            // collision originated in this batch rather
                            // than from an existing row.
                            return Err(DoogatError::singleton_violation(
                                type_name,
                                "<intra-batch>".to_string(),
                            ));
                        }
                    }
                }
                seen_singleton.insert(type_name, input_idx);
            }

            if let Some((prior_idx, conflict_key)) =
                find_intra_batch_duplicate(input, &schemas, &seen_unique)
            {
                match input.on_conflict {
                    crate::types::ConflictAction::Ignore => {
                        intra_dup_links[input_idx] = Some(prior_idx);
                        continue;
                    }
                    crate::types::ConflictAction::Error => {
                        // `values` is built from `value_to_unique_key`, which
                        // emits an empty string for non-scalar `Value` variants
                        // (List, Map, Null). Surfaced as-is in
                        // `extensions.values`; lossy for those variants but
                        // current typedefs constrain unique columns to scalars.
                        let (table, columns, values) = conflict_key;
                        return Err(DoogatError::unique_violation(table, columns, values));
                    }
                }
            }

            record_intra_batch_unique(input, &schemas, &mut seen_unique, input_idx);

            let (path, content) = self.prepare_create(
                input,
                &schemas,
                &mut next_counters,
                &mut partitioned_counters,
            )?;
            writes.push((input_idx, path, content));
        }

        // Phase 2: atomic commit
        self.commit_batch_creates(&writes, message)?;

        // Phase 3: index new writes and fill result slots
        self.index_batch_creates(&writes, &schemas, &mut results)?;

        // Resolve intra-batch duplicate links to the surviving row's payload.
        for (i, link) in intra_dup_links.iter().enumerate() {
            if let Some(j) = link {
                results[i] = results[*j].clone();
            }
        }

        Ok(results.into_iter().flatten().collect())
    }

    /// Build a single doogat for batch_create: generate ID, resolve defaults, serialize.
    ///
    /// PRD 00133: typed creates route through the unified
    /// `prepare_typed_insert_validate` and `build_data_doogat` helpers so the
    /// resulting `ParsedDoogat` has REFERENCES values in the reference zone
    /// (not frontmatter), FK validation queries the typedef target table
    /// (not the generic `doogats` index), and `allowed_values` rejects with
    /// the same wording the SQL path emits. Untyped creates keep the legacy
    /// "straight frontmatter" behavior to preserve PRD 00129 §T3 (CLI silent
    /// base-only creation).
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

        let schema = input
            .doogat_type
            .as_deref()
            .and_then(|t| schemas.iter().find(|s| s.table_name == t));

        let parsed = match schema {
            Some(schema) => self.build_typed_create(
                input,
                schema,
                &id,
                &path,
                next_counters,
                partitioned_counters,
            )?,
            None => self.build_untyped_create(input, &id, &path)?,
        };

        let content = parser::serialize(&parsed);
        Ok((path, content))
    }

    /// Typed-create branch of `prepare_create`. Stringifies input fields,
    /// delegates default + ENUM + FK validation to
    /// `prepare_typed_insert_validate`, enforces post-defaults NOT NULL, and
    /// builds the `ParsedDoogat` via `build_data_doogat` so REFERENCES
    /// values land in the reference zone.
    fn build_typed_create(
        &self,
        input: &BatchCreateInput,
        schema: &TableSchema,
        id: &DoogatId,
        path: &str,
        next_counters: &mut BareNextCounters,
        partitioned_counters: &mut PartitionedNextCounters,
    ) -> Result<ParsedDoogat> {
        let mut col_values = stringify_typed_input_fields(input, schema)?;

        let mut helper_counters = extract_helper_counters_for_table(
            &schema.table_name,
            next_counters,
            partitioned_counters,
        );

        prepare_typed_insert_validate(
            schema,
            &mut col_values,
            &mut helper_counters,
            &self.index.conn,
        )?;

        write_back_helper_counters(
            &schema.table_name,
            &helper_counters,
            next_counters,
            partitioned_counters,
        );

        Self::validate_typed_create_post_defaults(input, schema, &col_values)?;

        let title = self.resolve_create_title(input, Some(schema), id, &col_values)?;
        col_values.insert("title".to_string(), title);

        let ref_folder_types: std::collections::HashSet<String> = schema
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref())
            .filter(|ref_table| self.index.type_uses_folder(ref_table, &self.repo))
            .cloned()
            .collect();

        let mut parsed = build_data_doogat(
            id,
            schema,
            &col_values,
            &ref_folder_types,
            Some(&self.index.conn),
        );
        parsed.meta.tags = input.tags.clone();
        if let Some(ref body) = input.body {
            parsed.body = body.clone();
        }
        parsed.path = path.to_string();
        Ok(parsed)
    }

    /// Untyped-create branch: no typedef, so no defaults/zones/FK validation
    /// applies. Preserves the legacy `create_doogat`-style frontmatter dump
    /// for PRD 00129 §T3 (CLI silent base-only creation). Title is required
    /// here — there is no template to fall back on.
    fn build_untyped_create(
        &self,
        input: &BatchCreateInput,
        id: &DoogatId,
        path: &str,
    ) -> Result<ParsedDoogat> {
        let title = input
            .title
            .clone()
            .ok_or_else(|| DoogatError::not_null_violation("doogats", "title".to_string()))?;
        let meta = DoogatMeta {
            id: Some(id.clone()),
            title: Some(title),
            date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
            doogat_type: input.doogat_type.clone(),
            tags: input.tags.clone(),
            extra: input.fields.clone(),
        };
        Ok(ParsedDoogat {
            meta,
            body: input.body.clone().unwrap_or_default(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: path.to_string(),
            updated_at: None,
        })
    }

    /// Resolve the title for a batch create. Mirrors the `executeSql INSERT`
    /// path's title chain so the GraphQL surface (issue #13) and the SQL
    /// surface produce identical titles for the same input.
    ///
    /// - `Some(t)` from the caller is used verbatim.
    /// - `None` with a typedef that declares `title_template` renders the
    ///   template via the shared `resolve_insert_title` helper.
    /// - `None` with a typedef that has no template, or no typedef at all,
    ///   is rejected with `NOT_NULL_VIOLATION` on the title column.
    fn resolve_create_title(
        &self,
        input: &BatchCreateInput,
        schema: Option<&TableSchema>,
        id: &DoogatId,
        col_values: &std::collections::BTreeMap<String, String>,
    ) -> Result<String> {
        if let Some(t) = input.title.clone() {
            return Ok(t);
        }
        match schema {
            Some(s) if s.title_template.is_some() => Ok(crate::sql_engine::resolve_insert_title(
                id,
                s,
                col_values,
                Some(&self.index.conn),
            )),
            Some(s) => Err(DoogatError::not_null_violation(
                s.table_name.clone(),
                "title".to_string(),
            )),
            None => Err(DoogatError::not_null_violation(
                "doogats".to_string(),
                "title".to_string(),
            )),
        }
    }

    /// Phase 2: atomic commit for batch creates (no-op when writes is empty).
    fn commit_batch_creates(&self, writes: &[(usize, String, String)], message: &str) -> Result<()> {
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
            message,
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
    /// resolved. A required column is satisfied when `col_values` carries a
    /// non-empty value for it, when the typedef declares a `default_value`
    /// (already merged into `col_values` by `prepare_typed_insert_validate`),
    /// or — for the special `title` column — when the typedef declares a
    /// `title_template` (the title is synthesized at INSERT time, mirroring
    /// the SQL validator's exemption from PRD 00122).
    fn validate_typed_create_post_defaults(
        input: &BatchCreateInput,
        schema: &TableSchema,
        col_values: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let type_name = match input.doogat_type {
            Some(ref t) => t,
            None => return Ok(()),
        };

        for col in &schema.columns {
            if !col.required {
                continue;
            }
            // Title is enforced separately in `resolve_create_title`, which
            // accepts `Some(t)`, falls back to the typedef's `title_template`
            // when `None`, and emits its own `NOT_NULL_VIOLATION` when neither
            // is available. Skip it here so we don't double-emit.
            if col.name == "title" {
                continue;
            }
            if col.default_value.is_some() {
                continue;
            }
            if col_values.contains_key(&col.name) {
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
        let current_content = self.repo.read_file(&rel_path)?;
        let current = parser::parse(&current_content, &rel_path)?;
        let desired = parser::parse(content, &rel_path)?;
        let update = build_batch_update_from_replacement(id, &current, &desired);
        self.batch_update_with_message(&[update], message)?;
        Ok(())
    }

    /// Delete a doogat by ID. Returns broken backlinks `(source_id, source_path)`.
    ///
    /// Cascade behavior:
    /// - Junction table rows and dangling wikilinks in referencing files
    ///   are cleaned up atomically in a single git commit.
    /// - PRD 00129 §2: typed-table rows that reference the deleted id
    ///   through an `ON DELETE CASCADE` column are deleted recursively in
    ///   the same commit. Cycle detection rejects with `CASCADE_CYCLE`.
    pub fn delete_doogat(&self, id: &str, message: &str) -> Result<Vec<(String, String)>> {
        self.ensure_fresh()?;
        // Build the full cascade plan up front so the commit covers the
        // whole graph atomically (parent + every cascade-collected
        // descendant + their reference edits).
        let plan = self.build_cascade_delete_plan(id)?;
        self.execute_delete_plan(plan, id, message)
    }

    /// PRD 00129 §2: walk the CASCADE graph rooted at `id`, returning the
    /// ordered list of (id, path) pairs to delete. Cycle detection rejects
    /// with `CASCADE_CYCLE` listing the offending tables.
    fn build_cascade_delete_plan(&self, id: &str) -> Result<Vec<(String, String)>> {
        use std::collections::BTreeSet;
        let root_path = self.index.resolve_path(id)?;
        let mut ordered: Vec<(String, String)> = vec![(id.to_string(), root_path)];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        seen.insert(id.to_string());
        // Process FIFO so children of children land in a stable, depth-first
        // order. Cycle = revisiting a parent we've already enqueued; we
        // collect the tables involved for the error context.
        let mut cursor = 0;
        while cursor < ordered.len() {
            let parent = ordered[cursor].0.clone();
            cursor += 1;
            // RESTRICT check applies at every level: if any cascade-deleted
            // child has a RESTRICT-marked back-reference, the whole delete
            // rejects.
            self.index
                .check_restrict_blocks_delete(&self.repo, &parent)?;
            let children = self.index.collect_cascade_children(&self.repo, &parent)?;
            for (child_table, child_id) in children {
                if !seen.insert(child_id.clone()) {
                    return Err(DoogatError::cascade_cycle([child_table, parent.clone()]));
                }
                let child_path = match self.index.resolve_path(&child_id) {
                    Ok(p) => p,
                    Err(_) => continue, // child already gone? skip silently
                };
                ordered.push((child_id, child_path));
            }
        }
        Ok(ordered)
    }

    /// Execute a pre-collected cascade plan: collect ref edits, update the
    /// index, and commit every deletion + edit in a single batch.
    fn execute_delete_plan(
        &self,
        plan: Vec<(String, String)>,
        root_id: &str,
        message: &str,
    ) -> Result<Vec<(String, String)>> {
        use std::collections::BTreeSet;
        let broken = self.index.backlinking_doogat_paths(root_id)?;
        // Paths that will be deleted in this batch — we must not emit a
        // write edit for them (commit_batch can't both write and delete
        // the same path in one commit; git2 errors on the conflicting
        // index op). Edits to other backlinking files are still emitted.
        let delete_paths: BTreeSet<&str> = plan.iter().map(|(_, p)| p.as_str()).collect();
        let mut ref_edits: Vec<(String, String)> = Vec::new();
        for (id, path) in &plan {
            let edits = self.collect_ref_edits(id, path)?;
            for (p, c) in edits {
                if delete_paths.contains(p.as_str()) {
                    continue;
                }
                ref_edits.push((p, c));
            }
        }
        for (id, _path) in &plan {
            let doogat_type: Option<String> = self
                .index
                .conn
                .query_row(
                    "SELECT type FROM doogats WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .ok();
            self.index.remove_doogat(id)?;
            self.nosql_remove_doogat(id);
            if let Some(ref dtype) = doogat_type {
                if !dtype.is_empty() && dtype != "_typedef" {
                    let _ = self.index.conn.execute(
                        &format!("DELETE FROM \"{}\" WHERE id = ?1", dtype),
                        params![id],
                    );
                    self.index.cascade_junction_cleanup(&self.repo, dtype, id)?;
                }
            }
        }
        let writes: Vec<(&str, &str)> = ref_edits
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        let deletes: Vec<&str> = plan.iter().map(|(_, p)| p.as_str()).collect();
        self.repo.commit_batch(&writes, &deletes, message)?;
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

/// Stringify a typed `BatchCreateInput`'s fields into the `BTreeMap<String, String>`
/// shape `prepare_typed_insert_validate` expects. Reserved core columns
/// (id/title/type/date/created_at/updated_at/tags) are skipped — they don't
/// participate in zone routing or per-column validation. Structured values
/// (`List`/`Map`) on declared scalar columns reject up front; they cannot
/// satisfy `allowed_values` or FK checks and would silently disappear in
/// `value_to_comparable_string`.
fn stringify_typed_input_fields(
    input: &BatchCreateInput,
    schema: &TableSchema,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    for (key, val) in &input.fields {
        if RESERVED_TYPED_COLUMNS.contains(&key.as_str()) {
            continue;
        }
        if !schema.columns.iter().any(|c| &c.name == key) {
            // Pre-defaults check already rejected unknown columns; defensive
            // skip in case of future drift.
            continue;
        }
        match crate::types::Value::clone(val) {
            crate::types::Value::String(s) => {
                out.insert(key.clone(), s);
            }
            crate::types::Value::Number(n) => {
                out.insert(key.clone(), n.to_string());
            }
            crate::types::Value::Bool(b) => {
                out.insert(key.clone(), if b { "1".into() } else { "0".into() });
            }
            crate::types::Value::List(_) | crate::types::Value::Map(_) => {
                return Err(DoogatError::Validation(format!(
                    "field '{key}' has structured value (list/map) but typed column '{}.{key}' expects a scalar",
                    schema.table_name
                )));
            }
        }
    }
    Ok(out)
}

/// Copy the rows of `bare`/`partitioned` that target `table` into a
/// column-keyed `TypedInsertCounters` view the shared helper consumes.
/// Cross-input batch-aware counters keep advancing because callers write
/// the helper-side mutations back via `write_back_helper_counters`.
fn extract_helper_counters_for_table(
    table: &str,
    bare: &BareNextCounters,
    partitioned: &PartitionedNextCounters,
) -> TypedInsertCounters {
    let mut result = TypedInsertCounters::default();
    for ((t, col), val) in bare {
        if t == table {
            result.bare.insert(col.clone(), *val);
        }
    }
    for ((t, col, partition), val) in partitioned {
        if t == table {
            result
                .partitioned
                .insert((col.clone(), partition.clone()), *val);
        }
    }
    result
}

/// Inverse of `extract_helper_counters_for_table`: write the helper's
/// mutated counter view back into the service-side table-keyed maps so the
/// next input in the same batch sees the advanced counters.
fn write_back_helper_counters(
    table: &str,
    helper: &TypedInsertCounters,
    bare: &mut BareNextCounters,
    partitioned: &mut PartitionedNextCounters,
) {
    for (col, val) in &helper.bare {
        bare.insert((table.to_owned(), col.clone()), *val);
    }
    for ((col, partition), val) in &helper.partitioned {
        partitioned.insert((table.to_owned(), col.clone(), partition.clone()), *val);
    }
}

fn apply_field_updates(
    parsed: &mut ParsedDoogat,
    title: Option<&str>,
    tags: Option<&[String]>,
    doogat_type: Option<&str>,
    body: Option<&str>,
    extra: &ExtraFieldUpdates<'_>,
    schema: Option<&TableSchema>,
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

    // Schema-aware SET routing for typed doogats: REFERENCES columns must
    // land in the reference zone (`- col:: [[id]]`) so the materializer's
    // `extract_multi_reference_values` sees the new value. Falling back to a
    // frontmatter dump leaves the OLD reference line in place, which causes
    // `materialize_single` to re-INSERT the stale junction row even after
    // its DELETE pass — observed as task #2 of PRD 00134 cycle-1 review C1.
    //
    // Untyped doogats (no schema available) keep the legacy frontmatter dump
    // because there is no zone information to route by.
    if let Some(s) = schema {
        // Schema-aware UNSET routing: typed REFERENCES columns must clear
        // the reference-zone line too. Removing only from `parsed.meta.extra`
        // (above) leaves the old `- col:: [[id]]` line, which
        // `materialize_single → populate_junction_tables` re-INSERTs as a
        // stale junction row. Route through `apply_updates_to_doogat` with
        // empty value, which writes `- col:: [[]]` and gets stripped by the
        // empty-value filter in `extract_multi_reference_values` (PRD 00134
        // cycle-1 fix `58405f4`). PRD 00134 cycle-2 review C2 task #1.
        let unset_blanks: std::collections::BTreeMap<String, String> = extra
            .unset
            .iter()
            .filter(|key| {
                s.columns
                    .iter()
                    .any(|c| &c.name == *key && c.references.is_some())
            })
            .map(|key| (key.clone(), String::new()))
            .collect();
        if !unset_blanks.is_empty() {
            apply_updates_to_doogat(parsed, s, &unset_blanks);
        }
        let stringified = stringify_extra_set_for_schema(extra.set, s);
        if !stringified.is_empty() {
            apply_updates_to_doogat(parsed, s, &stringified);
        }
        // Frontmatter set for any keys that the schema doesn't know about
        // (e.g. user-supplied untyped extras alongside typed columns).
        for (key, value) in extra.set {
            if !s.columns.iter().any(|c| &c.name == key) {
                parsed.meta.extra.insert(key.clone(), value.clone());
            }
        }
    } else {
        for (key, value) in extra.set {
            parsed.meta.extra.insert(key.clone(), value.clone());
        }
    }
}

/// Convert the typed-update `set` map (`Value`-valued) into the
/// `String`-valued shape `apply_updates_to_doogat` expects, dropping keys
/// that aren't declared columns on `schema`. Mirrors
/// `stringify_typed_input_fields` (used for create) but for update; rejects
/// nothing on List/Map (those keys are silently routed to frontmatter by
/// the caller, preserving the legacy "extra fields just go to frontmatter"
/// behavior for non-typed keys).
fn stringify_extra_set_for_schema(
    set: &std::collections::BTreeMap<String, crate::types::Value>,
    schema: &TableSchema,
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (key, value) in set {
        if !schema.columns.iter().any(|c| &c.name == key) {
            continue;
        }
        match value {
            crate::types::Value::String(s) => {
                out.insert(key.clone(), s.clone());
            }
            crate::types::Value::Number(n) => {
                out.insert(key.clone(), n.to_string());
            }
            crate::types::Value::Bool(b) => {
                out.insert(key.clone(), if *b { "1".into() } else { "0".into() });
            }
            crate::types::Value::List(_) | crate::types::Value::Map(_) => {
                // Structured values can't satisfy a typed scalar column; let
                // the caller surface validation errors via the existing
                // `validate_fields_with_schemas` pass instead of silently
                // dropping the update here.
            }
        }
    }
    out
}

fn parsed_fields(parsed: &ParsedDoogat) -> std::collections::BTreeMap<String, crate::types::Value> {
    let mut fields = parsed.meta.extra.clone();
    for field in &parsed.inline_fields {
        fields.insert(
            field.key.clone(),
            crate::types::Value::String(field.value.clone()),
        );
    }
    fields
}

fn build_batch_update_from_replacement(
    id: &str,
    current: &ParsedDoogat,
    desired: &ParsedDoogat,
) -> BatchUpdateInput {
    let current_fields = parsed_fields(current);
    let desired_fields = parsed_fields(desired);
    let unset_fields = current_fields
        .keys()
        .filter(|key| !desired_fields.contains_key(*key))
        .cloned()
        .collect();

    BatchUpdateInput {
        id: id.to_string(),
        title: desired.meta.title.clone(),
        body: Some(desired.body.clone()),
        tags: Some(desired.meta.tags.clone()),
        doogat_type: desired.meta.doogat_type.clone(),
        fields: Some(desired_fields),
        unset_fields: Some(unset_fields),
    }
}

fn build_batch_create_from_parsed(parsed: &ParsedDoogat) -> BatchCreateInput {
    BatchCreateInput {
        title: parsed.meta.title.clone(),
        body: Some(parsed.body.clone()),
        tags: parsed.meta.tags.clone(),
        doogat_type: parsed.meta.doogat_type.clone(),
        fields: parsed_fields(parsed),
        on_conflict: crate::types::ConflictAction::Error,
    }
}
