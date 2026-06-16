use crate::app_contract::{AppOutput, UpdateCommand};
use crate::error::{DoogatError, Result};
use crate::parser;
use crate::sql_engine::apply_updates_to_doogat;
use crate::types::{BatchUpdateInput, ParsedDoogat, TableSchema};

use crate::traits::{GitBackend, IndexPort};

use super::{DoogatService, ExtraFieldUpdates};

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
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

        // PRD 00157: resolve the *result* type (the type after this update) —
        // the arg type wins, otherwise the doogat keeps its current type. A
        // retype into a registered (possibly SINGLETON) typedef is the gap
        // this closes, so schema loading is broadened from the pre-00157 gate
        // on the *current* type to the result type. Without this an untyped
        // doogat retyped into a SINGLETON typedef would skip the singleton
        // check and never materialize into the typed table.
        let result_type: Option<String> = doogat_type
            .map(|t| t.to_string())
            .or_else(|| parsed.meta.doogat_type.clone());

        // Resolve schemas BEFORE applying field updates so typed-column SETs
        // route to the correct zone (REFERENCES → reference section, etc.).
        // PRD 00134 cycle-1 review C1 task #2.
        let schemas = if result_type.is_some() {
            Some(self.list_type_schemas()?)
        } else {
            None
        };
        let schema = schemas.as_ref().and_then(|all| {
            result_type
                .as_deref()
                .and_then(|t| all.iter().find(|s| s.table_name == t))
        });

        // PRD 00157: when the result type is a registered SINGLETON typedef,
        // run the constraint-check → commit → reindex → store_head window
        // inside one BEGIN IMMEDIATE transaction (mirrors `update_doogat_raw`
        // and the create paths) so a cross-process loser surfaces a structured
        // SINGLETON_VIOLATION instead of a raw materializer error. Non-SINGLETON
        // updates run with no transaction and pay no new cost.
        let is_singleton = schema.map(|s| s.singleton).unwrap_or(false);

        let mut write = || -> Result<ParsedDoogat> {
            // Singleton check on the *result* type (no-op for unregistered or
            // non-SINGLETON types). First DB read inside the window, so the
            // loser's check runs only after the winner's COMMIT releases the
            // write lock and its row is visible.
            if let (Some(type_name), Some(all)) = (result_type.as_deref(), schemas.as_ref()) {
                if all.iter().any(|s| s.table_name == type_name) {
                    self.check_singleton_update_constraint(id, type_name, all)?;
                }
            }

            // Validate the user-supplied SET fields BEFORE routing them. After
            // routing, REFERENCES values land in the reference zone, hidden
            // from the `parsed.meta.extra`-based validator. We validate against
            // the input map directly so FK/allowed_values rejections still
            // fire on the typed UPDATE path. Kept keyed on the *current* type
            // (PRD 00157 design: retype field-validation behavior unchanged).
            let has_field_changes = !extra.set.is_empty() || !extra.unset.is_empty();
            if has_field_changes {
                if let (Some(type_name), Some(all)) =
                    (parsed.meta.doogat_type.as_deref(), schemas.as_ref())
                {
                    self.validate_fields_with_schemas(all, type_name, extra.set)?;
                }
            }

            apply_field_updates(&mut parsed, title, tags, doogat_type, body, extra, schema);

            let new_content = parser::serialize(&parsed);
            self.repo
                .commit_file(&path, &new_content, &format!("update doogat {id}"))?;
            let mut updated =
                self.reindex_and_rematerialize(&new_content, &path, schemas.as_deref())?;
            // Sync stored HEAD to avoid spurious incremental_reindex on next call
            self.index.store_head(&self.repo.head_oid()?.0)?;
            updated.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
            Ok(updated)
        };

        if is_singleton {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), write)
        } else {
            write()
        }
    }

    /// App facade entrypoint: update a doogat from an `UpdateCommand`.
    /// Mirrors `DoogatService::create`. Delegates to `update_doogat_parsed`
    /// and wraps the result in `AppOutput`. Update emits no warnings today,
    /// so `warnings` is always empty; the envelope keeps the warning channel
    /// available for callers.
    pub fn update(&self, cmd: UpdateCommand) -> Result<AppOutput<ParsedDoogat>> {
        let extra = ExtraFieldUpdates {
            set: &cmd.fields,
            unset: &cmd.unset_fields,
        };
        let value = self.update_doogat_parsed(
            &cmd.id,
            cmd.title.as_deref(),
            cmd.tags.as_deref(),
            cmd.doogat_type.as_deref(),
            cmd.body.as_deref(),
            &extra,
        )?;
        Ok(AppOutput {
            value,
            warnings: Vec::new(),
        })
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

        // PRD 00157: if any update's result type is a registered SINGLETON
        // typedef, run prepare-all → commit_batch → reindex-each → store_head
        // inside one BEGIN IMMEDIATE window so the per-update singleton check
        // (`prepare_update`) and the commit are atomic across processes
        // (mirrors `batch_create_with_message`). Batches with no
        // SINGLETON-targeting update run unwrapped — no new contention.
        let any_singleton = self.batch_update_targets_singleton(updates, &schemas)?;

        let body = || -> Result<Vec<ParsedDoogat>> {
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
            self.repo.commit_batch(&write_refs, &[], message)?;

            // Phase 3: re-parse, index, rematerialize, return
            let mut results = Vec::with_capacity(updates.len());
            for (path, new_content) in &writes {
                let parsed = self.reindex_and_rematerialize(new_content, path, Some(&schemas))?;
                results.push(parsed);
            }

            // Sync stored HEAD to avoid spurious incremental_reindex on next call
            self.index.store_head(&self.repo.head_oid()?.0)?;

            Ok(results)
        };

        if any_singleton {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), body)
        } else {
            body()
        }
    }

    /// PRD 00157: true when any update's *result* type (the arg type, else the
    /// stored type of `u.id`) names a registered SINGLETON typedef. Resolves
    /// the result type by re-parsing the stored file exactly as
    /// `prepare_update` derives `final_type`, so the pre-wrap scan and the
    /// in-window per-update check never disagree.
    fn batch_update_targets_singleton(
        &self,
        updates: &[BatchUpdateInput],
        schemas: &[TableSchema],
    ) -> Result<bool> {
        for u in updates {
            let result_type = match u.doogat_type.as_deref() {
                Some(t) => Some(t.to_string()),
                None => {
                    let path = self.index.resolve_path(&u.id)?;
                    let content = self.repo.read_file(&path)?;
                    parser::parse(&content, &path)?.meta.doogat_type
                }
            };
            if let Some(t) = result_type {
                if schemas
                    .iter()
                    .find(|s| s.table_name == t)
                    .map(|s| s.singleton)
                    .unwrap_or(false)
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
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

    /// Update a doogat from raw content (for FFI consumers).
    ///
    /// PRD 00139 cycle-3 task #1: raw-frontmatter semantics. The desired
    /// markdown replaces the stored content verbatim — title omitted from
    /// the new frontmatter clears the stored title, the new `date:` value
    /// overwrites the old, custom keys round-trip. We do not route through
    /// `BatchUpdateInput`, which silently drops top-level meta fields and
    /// re-applies "title omitted = keep old".
    ///
    /// When the desired markdown targets a registered SINGLETON typedef,
    /// `check_singleton_update_constraint` still fires before the commit
    /// lands so cross-row singleton invariants stay enforced.
    pub fn update_doogat_raw(&self, id: &str, content: &str, message: &str) -> Result<()> {
        self.ensure_fresh()?;
        let rel_path = self.index.resolve_path(id)?;
        let desired = parser::parse(content, &rel_path)?;
        let schemas = self.list_type_schemas()?;

        // PRD 00140: an update into a registered SINGLETON typedef runs
        // its constraint-check → commit → index window inside a
        // `BEGIN IMMEDIATE` transaction so a cross-process race surfaces a
        // structured SINGLETON_VIOLATION on the losing writer.
        let is_singleton = desired
            .meta
            .doogat_type
            .as_deref()
            .and_then(|t| schemas.iter().find(|s| s.table_name == t))
            .map(|s| s.singleton)
            .unwrap_or(false);

        let write = || -> Result<()> {
            if let Some(type_name) = desired.meta.doogat_type.as_deref() {
                if schemas.iter().any(|s| s.table_name == type_name) {
                    self.check_singleton_update_constraint(id, type_name, &schemas)?;
                    // PRD 00134 batch-end follow-up: mirror create_doogat_raw's
                    // FK + allowed_values pre-check so update cannot bypass
                    // validation that create enforces.
                    let fields = parsed_fields(&desired);
                    self.validate_fields_with_schemas(&schemas, type_name, &fields)?;
                }
            }

            self.repo.commit_file(&rel_path, content, message)?;
            let _ = self.reindex_and_rematerialize(content, &rel_path, Some(&schemas))?;
            self.index.store_head(&self.repo.head_oid()?.0)?;
            Ok(())
        };

        if is_singleton {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), write)
        } else {
            write()
        }
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

pub(super) fn parsed_fields(
    parsed: &ParsedDoogat,
) -> std::collections::BTreeMap<String, crate::types::Value> {
    let mut fields = parsed.meta.extra.clone();
    for field in &parsed.inline_fields {
        fields.insert(
            field.key.clone(),
            crate::types::Value::String(field.value.clone()),
        );
    }
    fields
}
