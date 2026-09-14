use crate::app_contract::{summarize_reindex_warnings, AppOutput, UpdateCommand};
use crate::error::{DoogatError, Result};
use crate::parser;
use crate::sql_engine::apply_updates_to_doogat;
use crate::types::{BatchUpdateInput, CommitHash, ParsedDoogat, TableSchema};

use crate::traits::{GitBackend, IndexPort};

use super::validation::UniqueGroupKey;
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

        // FT-5 (hazard H5): whether this update needs a BEGIN IMMEDIATE
        // transaction is decided BEFORE any lock is taken (SQLite-outer,
        // git-inner: the transaction can never be opened under the git
        // lock). This read is only a preview of the stored type; the
        // authoritative read happens inside the write lock below.
        let preview = parser::parse(&self.repo.read_file(&path)?, &path)?;
        let preview_type = doogat_type.or(preview.meta.doogat_type.as_deref());
        let preview_schemas = if preview_type.is_some() {
            Some(self.list_type_schemas()?)
        } else {
            None
        };
        let needs_transaction =
            needs_transaction_lock(find_schema(preview_schemas.as_deref(), preview_type));

        let write = || -> Result<ParsedDoogat> {
            #[cfg(test)]
            if let Some(pause) = tests::AT_PREVIEW_TO_LOCK.take() {
                pause();
            }
            // Locked read → modify → commit. Two writers on the same doogat
            // used to read the same HEAD outside the lock and the second
            // commit silently dropped the first's field. Everything under the
            // lock is a git read, a SQLite read, or the one git commit;
            // transaction acquisition happens before it, and reindexing
            // runs after release.
            let (commit, new_content, schemas) = self.repo.with_write_lock(|| {
                self.commit_locked_update(
                    id,
                    &path,
                    doogat_type,
                    extra,
                    needs_transaction,
                    |parsed, schema| {
                        apply_field_updates(parsed, title, tags, doogat_type, body, extra, schema)
                    },
                )
            })?;
            #[cfg(test)]
            if let Some(pause) = tests::AT_COMMIT_TO_STORE_HEAD.take() {
                pause();
            }

            let mut updated =
                self.reindex_and_rematerialize(&new_content, &path, schemas.as_deref())?;
            // Store THIS update's commit as the indexed HEAD, never an
            // unrelated later HEAD a concurrent writer may have moved it to.
            self.index.store_head(&commit.0)?;
            updated.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
            Ok(updated)
        };

        if needs_transaction {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), write)
        } else {
            write()
        }
    }

    /// FT-5 (hazard H5): the locked read → check → modify → commit of one
    /// doogat. Runs inside `GitBackend::with_write_lock`; everything here is
    /// a git read, a SQLite read, or the one git commit. `apply` merges the
    /// caller's field updates into the freshly read file under the resolved
    /// result-type schema. Returns the commit, the committed content and the
    /// schemas the caller reindexes against after the lock is released.
    fn commit_locked_update(
        &self,
        id: &str,
        path: &str,
        doogat_type: Option<&str>,
        extra: &ExtraFieldUpdates<'_>,
        needs_transaction: bool,
        apply: impl FnOnce(&mut ParsedDoogat, Option<&TableSchema>),
    ) -> Result<(CommitHash, String, Option<Vec<TableSchema>>)> {
        let content = self.repo.read_file(path)?;
        let mut parsed = parser::parse(&content, path)?;

        // PRD 00157: resolve the *result* type (the type after this
        // update) — the arg type wins, otherwise the doogat keeps its
        // current type. A retype into a registered (possibly
        // SINGLETON) typedef is the gap this closes, so schema loading
        // is broadened from the pre-00157 gate on the *current* type
        // to the result type. Without this an untyped doogat retyped
        // into a SINGLETON typedef would skip the singleton check and
        // never materialize into the typed table.
        let result_type: Option<String> = doogat_type
            .map(|t| t.to_string())
            .or_else(|| parsed.meta.doogat_type.clone());

        // Resolve schemas BEFORE applying field updates so typed-column
        // SETs route to the correct zone (REFERENCES → reference
        // section, etc.). PRD 00134 cycle-1 review C1 task #2.
        let schemas = if result_type.is_some() {
            Some(self.list_type_schemas()?)
        } else {
            None
        };
        let schema = find_schema(schemas.as_deref(), result_type.as_deref());

        // A concurrent retype or CREATE TABLE may have raised the lock
        // requirement since the preview. Refuse loudly rather than run
        // a SINGLETON/UNIQUE check outside the transaction it needs;
        // the caller retries and the preview re-derives it.
        if !needs_transaction && needs_transaction_lock(schema) {
            return Err(DoogatError::Conflict(format!(
                "type or schema of doogat {id} changed during the update; retry"
            )));
        }

        // Singleton check on the *result* type (no-op for unregistered
        // or non-SINGLETON types). First DB read inside the window, so
        // the loser's check runs only after the winner's COMMIT
        // releases the write lock and its row is visible.
        if let (Some(type_name), Some(all)) = (result_type.as_deref(), schemas.as_ref()) {
            if schema.is_some() {
                self.check_singleton_update_constraint(id, type_name, all)?;

                // FT-1: pre-commit UNIQUE check against the merged
                // field set (existing fields overlaid with this
                // update's SET/UNSET), excluding this row, so a UNIQUE
                // collision is rejected before materialize_single's
                // `INSERT OR REPLACE` would otherwise evict the other
                // row from the typed table.
                let mut merged = parsed_fields(&parsed);
                for key in extra.unset {
                    merged.remove(key);
                }
                for (key, value) in extra.set {
                    merged.insert(key.clone(), value.clone());
                }
                self.check_unique_constraints_for_update(id, type_name, &merged, all)?;
            }
        }

        // Validate the user-supplied SET fields BEFORE routing them.
        // After routing, REFERENCES values land in the reference zone,
        // hidden from the `parsed.meta.extra`-based validator. We
        // validate against the input map directly so FK/allowed_values
        // rejections still fire on the typed UPDATE path. Kept keyed on
        // the *current* type (PRD 00157 design: retype field-validation
        // behavior unchanged).
        let has_field_changes = !extra.set.is_empty() || !extra.unset.is_empty();
        if has_field_changes {
            if let (Some(type_name), Some(all)) =
                (parsed.meta.doogat_type.as_deref(), schemas.as_ref())
            {
                self.validate_fields_with_schemas(all, type_name, extra.set)?;
            }
        }

        apply(&mut parsed, schema);

        let new_content = parser::serialize(&parsed);
        let commit = self
            .repo
            .commit_file(path, &new_content, &format!("update doogat {id}"))?;
        Ok((commit, new_content, schemas))
    }

    /// App facade entrypoint: update a doogat from an `UpdateCommand`.
    /// Mirrors `DoogatService::create`. Delegates to `update_doogat_parsed`
    /// and wraps the result in `AppOutput`. `warnings` carries at most one
    /// `REINDEX_SKIPPED_FILES` entry, summarizing any files the up-front
    /// freshness reindex had to skip.
    pub fn update(&self, cmd: UpdateCommand) -> Result<AppOutput<ParsedDoogat>> {
        let reindex_warnings = self.ensure_fresh()?;
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
            warnings: summarize_reindex_warnings(reindex_warnings)
                .into_iter()
                .collect(),
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

        // PRD 00157 (+ FT-1 rework): resolve each update's result-type lock
        // requirements once. A SINGLETON target, or a target with
        // unique_together groups, means the check-to-materialize window
        // must run inside one BEGIN IMMEDIATE transaction (mirrors
        // `batch_create_with_message`) so a cross-process race surfaces a
        // structured violation on the loser instead of letting
        // `INSERT OR REPLACE` materialization silently evict a row. Batches
        // with no such target run unwrapped — no new contention.
        let lock_targets = self.resolve_batch_lock_targets(updates, &schemas)?;
        let any_transaction_needed = lock_targets.iter().any(|(s, u)| s.is_some() || *u);

        let body = || -> Result<Vec<ParsedDoogat>> {
            // Phase 1: prepare all writes (fail-fast, no side effects). Track
            // intra-batch SINGLETON targets so a second update retyping into the
            // same SINGLETON typedef rejects BEFORE commit_batch lands —
            // create-path parity with `batch_create_body`'s `seen_singleton`
            // (PRD 00157 doubt-review #2). `reject_duplicate_update_ids` already
            // guarantees distinct ids, so two updates sharing a singleton target
            // are two rows contending for the one slot.
            //
            // FT-1 rework: also track intra-batch UNIQUE candidate keys
            // (type, group_columns, values) the same way, since two updates
            // in one batch can each pass `prepare_update`'s per-item DB check
            // (which only sees the pre-batch materialized table) and still
            // collide with each other once both are materialized in Phase 3.
            let mut writes: Vec<(String, String)> = Vec::with_capacity(updates.len());
            let mut seen_singleton: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            let mut seen_unique: std::collections::HashSet<UniqueGroupKey> =
                std::collections::HashSet::new();
            for (i, update) in updates.iter().enumerate() {
                if let Some(type_name) = lock_targets[i].0.as_deref() {
                    if !seen_singleton.insert(type_name) {
                        return Err(DoogatError::singleton_violation(
                            type_name.to_string(),
                            "<intra-batch>".to_string(),
                        ));
                    }
                }
                let (path, content, unique_candidates) = self.prepare_update(update, &schemas)?;
                for key in &unique_candidates {
                    if seen_unique.contains(key) {
                        let (table, columns, values) = key.clone();
                        return Err(DoogatError::unique_violation(table, columns, values));
                    }
                }
                seen_unique.extend(unique_candidates);
                writes.push((path, content));
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

        if any_transaction_needed {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), body)
        } else {
            body()
        }
    }

    /// PRD 00157 (+ FT-1 rework): resolve each update's *result* type's lock
    /// requirements — the arg type, else the stored type of `u.id` — in one
    /// pass. Element `.0` is `Some(type_name)` only when the result type is a
    /// registered SINGLETON typedef (drives the intra-batch SINGLETON
    /// collision check); `.1` is `true` when the result type has at least
    /// one unique_together group (drives the intra-batch UNIQUE collision
    /// check). Resolves the result type by re-parsing the stored file
    /// exactly as `prepare_update` derives `final_type`, so the pre-wrap scan
    /// and the in-window per-update check never disagree. One pass drives
    /// both the conditional BEGIN IMMEDIATE wrap (any lock target → wrap) and
    /// the intra-batch collision checks (PRD 00157 doubt-review #2).
    fn resolve_batch_lock_targets(
        &self,
        updates: &[BatchUpdateInput],
        schemas: &[TableSchema],
    ) -> Result<Vec<(Option<String>, bool)>> {
        let mut targets = Vec::with_capacity(updates.len());
        for u in updates {
            let result_type = match u.doogat_type.as_deref() {
                Some(t) => Some(t.to_string()),
                None => {
                    let path = self.index.resolve_path(&u.id)?;
                    let content = self.repo.read_file(&path)?;
                    parser::parse(&content, &path)?.meta.doogat_type
                }
            };
            let schema = result_type
                .as_deref()
                .and_then(|t| schemas.iter().find(|s| s.table_name == t));
            let singleton = schema.filter(|s| s.singleton).map(|s| s.table_name.clone());
            let has_unique = schema
                .and_then(|s| s.unique_together.as_ref())
                .map(|g| !g.is_empty())
                .unwrap_or(false);
            targets.push((singleton, has_unique));
        }
        Ok(targets)
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
    /// Returns `(path, new_content, unique_candidates)` without side
    /// effects. `unique_candidates` is this row's `(type, group_columns,
    /// values)` keys for every fully-resolvable unique_together group under
    /// the merged field set — the caller (`batch_update_with_message`) uses
    /// it to catch two updates in the same batch racing for the same UNIQUE
    /// tuple, since each alone passes the per-item DB check below (neither
    /// row is materialized until Phase 3).
    ///
    /// Routes typed REFERENCES-column SETs to the reference zone via the
    /// shared `apply_field_updates` helper so the auto-junction stays in
    /// sync after a `batch_update` mutation. PRD 00134 cycle-1 review C1
    /// task #2.
    fn prepare_update(
        &self,
        update: &BatchUpdateInput,
        schemas: &[TableSchema],
    ) -> Result<(String, String, Vec<UniqueGroupKey>)> {
        let path = self.index.resolve_path(&update.id)?;
        let content = self.repo.read_file(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        let final_type = update
            .doogat_type
            .as_deref()
            .or(parsed.meta.doogat_type.as_deref());
        let mut unique_candidates = Vec::new();
        if let Some(final_type) = final_type {
            let final_schema = match schemas.iter().find(|s| s.table_name == final_type) {
                Some(s) => s,
                None => return Err(DoogatError::type_not_registered(final_type.to_string())),
            };
            self.check_singleton_update_constraint(&update.id, final_type, schemas)?;

            // FT-1: same pre-commit UNIQUE check as `update_doogat_parsed`,
            // against the merged field set (existing fields overlaid with
            // this update's SET/UNSET), excluding this row.
            let mut merged = parsed_fields(&parsed);
            if let Some(unset) = update.unset_fields.as_ref() {
                for key in unset {
                    merged.remove(key);
                }
            }
            if let Some(set) = update.fields.as_ref() {
                for (key, value) in set {
                    merged.insert(key.clone(), value.clone());
                }
            }
            let effective_fields =
                self.merge_row_snapshot_for_unique_check(&update.id, final_type, &merged, schemas)?;
            self.check_effective_unique_fields(
                &update.id,
                final_type,
                final_schema,
                &effective_fields,
            )?;
            unique_candidates =
                Self::unique_group_candidates(final_type, final_schema, &effective_fields);
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
        Ok((path, new_content, unique_candidates))
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
    /// lands so cross-row singleton invariants stay enforced. FT-1 rework:
    /// the same is now true for a registered typedef's unique_together
    /// groups — this raw lane previously had no UNIQUE pre-check at all,
    /// unlike `create_doogat_raw`.
    pub fn update_doogat_raw(&self, id: &str, content: &str, message: &str) -> Result<()> {
        self.ensure_fresh()?;
        let rel_path = self.index.resolve_path(id)?;
        let desired = parser::parse(content, &rel_path)?;
        let schemas = self.list_type_schemas()?;

        let desired_schema = desired
            .meta
            .doogat_type
            .as_deref()
            .and_then(|t| schemas.iter().find(|s| s.table_name == t));

        // PRD 00140 (+ FT-1 rework): an update into a registered SINGLETON
        // typedef, or one with unique_together groups, runs its
        // constraint-check → commit → index window inside a
        // `BEGIN IMMEDIATE` transaction so a cross-process race surfaces a
        // structured violation on the losing writer instead of a silent
        // `INSERT OR REPLACE` eviction.
        let is_singleton = desired_schema.map(|s| s.singleton).unwrap_or(false);
        let has_unique_groups = desired_schema
            .and_then(|s| s.unique_together.as_ref())
            .map(|g| !g.is_empty())
            .unwrap_or(false);
        let needs_transaction = is_singleton || has_unique_groups;

        let write = || -> Result<()> {
            if let Some(type_name) = desired.meta.doogat_type.as_deref() {
                if schemas.iter().any(|s| s.table_name == type_name) {
                    self.check_singleton_update_constraint(id, type_name, &schemas)?;
                    // PRD 00134 batch-end follow-up: mirror create_doogat_raw's
                    // FK + allowed_values pre-check so update cannot bypass
                    // validation that create enforces.
                    let fields = parsed_fields(&desired);
                    // FT-1: raw update replaces the stored content wholesale,
                    // so `fields` (desired's own frontmatter + inline fields)
                    // is already the full post-update field set for this
                    // check — no SET/UNSET overlay needed, unlike the
                    // `update`/`batch_update` lanes.
                    self.check_unique_constraints_for_update(id, type_name, &fields, &schemas)?;
                    self.validate_fields_with_schemas(&schemas, type_name, &fields)?;
                }
            }

            self.repo.commit_file(&rel_path, content, message)?;
            let _ = self.reindex_and_rematerialize(content, &rel_path, Some(&schemas))?;
            self.index.store_head(&self.repo.head_oid()?.0)?;
            Ok(())
        };

        if needs_transaction {
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

fn find_schema<'a>(
    schemas: Option<&'a [TableSchema]>,
    type_name: Option<&str>,
) -> Option<&'a TableSchema> {
    let all = schemas?;
    let t = type_name?;
    all.iter().find(|s| s.table_name == t)
}

/// PRD 00157 + FT-1 rework: an update whose result type is a registered
/// SINGLETON typedef, or one with unique_together groups, must run its
/// check → commit → reindex → store_head window inside one BEGIN IMMEDIATE
/// transaction so a cross-process loser surfaces a structured violation
/// instead of a raw materializer error or an `INSERT OR REPLACE` eviction.
/// Other updates run with no transaction and pay no new cost.
fn needs_transaction_lock(schema: Option<&TableSchema>) -> bool {
    match schema {
        Some(s) => s.singleton || s.unique_together.as_ref().is_some_and(|g| !g.is_empty()),
        None => false,
    }
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

/// FT-5 (hazard H5): two concurrent single-row field updates on ONE doogat
/// must both land in committed HEAD, or the loser must refuse loudly (an
/// `Err` to its caller). Whole-file last-writer-wins - the stored file read
/// outside the write lock, then committed with no expected-parent check - is
/// the silent loss these tests pin. Each writer is its own thread with its
/// own service handle, released per round by a shared barrier so both start
/// together (the barrier makes the race likely each round, not certain);
/// durability is judged against the committed HEAD tree, never the returned
/// values.
#[cfg(test)]
mod tests {
    use super::parsed_fields;
    use crate::parser;
    use crate::service::{DoogatService, ExtraFieldUpdates};
    use crate::types::Value;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::{mpsc, Arc, Barrier};
    use std::time::Duration;

    thread_local! {
        /// Interleaving seam for `update_doogat_parsed`: taken and run ONCE on
        /// the updating thread after the preview has fixed `needs_transaction`
        /// and immediately before the git write lock is taken. Test-only
        /// (`cfg(test)`), thread-local so a test can pin one updater without
        /// touching others; never set in production builds.
        pub(super) static AT_PREVIEW_TO_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            const { std::cell::RefCell::new(None) };
        /// Interleaving seam for `update_doogat_parsed`: taken and run ONCE on
        /// the updating thread after `with_write_lock` has returned (lock
        /// released, commit durable) and before the reindex and `store_head`.
        /// Same test-only, thread-local contract as `AT_PREVIEW_TO_LOCK`.
        pub(super) static AT_COMMIT_TO_STORE_HEAD: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Rounds of the two-writer race; every round fires both before waiting.
    const ROUNDS: usize = 8;
    /// Bound on waiting for a writer's report; the repo write lock itself
    /// times out after 10 s, so a hung writer surfaces as a failure, not a hang.
    const ROUND_TIMEOUT: Duration = Duration::from_secs(30);

    type Report = (&'static str, Result<(), String>);

    /// Spawn one writer thread with its own service handle. Each round it
    /// waits on `start`, sets `key=<key>-<round>` on `id`, and reports the
    /// outcome on `report`. Never panics: a failed open is reported as a
    /// refusal every round so the barrier can't hang the test.
    fn spawn_writer(
        repo: &Path,
        id: &str,
        key: &'static str,
        start: Arc<Barrier>,
        report: mpsc::Sender<Report>,
    ) {
        let repo = repo.to_path_buf();
        let id = id.to_string();
        std::thread::spawn(move || {
            let svc = DoogatService::open(&repo).map_err(|e| e.to_string());
            for round in 1..=ROUNDS {
                let mut set = BTreeMap::new();
                set.insert(key.to_string(), Value::String(format!("{key}-{round}")));
                let extra = ExtraFieldUpdates {
                    set: &set,
                    unset: &[],
                };
                start.wait();
                let outcome = svc.as_ref().map_err(Clone::clone).and_then(|svc| {
                    svc.update_doogat(&id, None, None, None, None, &extra)
                        .map_err(|e| e.to_string())
                });
                if report.send((key, outcome)).is_err() {
                    return;
                }
            }
        });
    }

    /// Race writers `a` and `b` on `id` for `ROUNDS` rounds. Returns one line
    /// per successful writer whose field is missing from HEAD; empty means
    /// the contract held.
    fn race_two_writers(svc: &DoogatService, id: &str) -> Vec<String> {
        let start = Arc::new(Barrier::new(3));
        let (tx, rx) = mpsc::channel();
        spawn_writer(svc.repo_path(), id, "a", start.clone(), tx.clone());
        spawn_writer(svc.repo_path(), id, "b", start.clone(), tx);

        let path = format!("ddb/{id}.md");
        let mut lost = Vec::new();
        for round in 1..=ROUNDS {
            start.wait();
            let outcomes = [
                rx.recv_timeout(ROUND_TIMEOUT)
                    .expect("first writer did not report within the round timeout"),
                rx.recv_timeout(ROUND_TIMEOUT)
                    .expect("second writer did not report within the round timeout"),
            ];
            assert!(
                outcomes.iter().any(|(_, o)| o.is_ok()),
                "round {round}: both writers refused, the round exercised nothing: {outcomes:?}"
            );

            let head = svc
                .repo
                .read_file(&path)
                .expect("read the committed doogat from HEAD");
            let committed =
                parsed_fields(&parser::parse(&head, &path).expect("parse committed doogat"));
            for (key, outcome) in &outcomes {
                if outcome.is_err() {
                    // Loud refusal: this writer's field is not expected this round.
                    continue;
                }
                let expected = Value::String(format!("{key}-{round}"));
                if committed.get(*key) != Some(&expected) {
                    lost.push(format!(
                        "round {round}: writer {key} returned Ok but HEAD holds {key}={:?} \
                         (expected {expected:?}); committed fields={committed:?}",
                        committed.get(*key)
                    ));
                }
            }
        }
        lost
    }

    #[test]
    fn concurrent_untyped_field_updates_on_one_doogat_keep_both_fields_or_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DoogatService::init(tmp.path()).expect("init repo");
        let id = svc
            .create_doogat("H5 shared row", &[], None, "")
            .expect("seed create");

        let lost = race_two_writers(&svc, &id);
        assert!(
            lost.is_empty(),
            "H5 fired (untyped): a successful single-row update vanished from HEAD, so a \
             concurrent field update is whole-file last-writer-wins. Losing rounds:\n{}",
            lost.join("\n")
        );
    }

    #[test]
    fn concurrent_typed_column_updates_on_one_doogat_keep_both_columns_or_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = DoogatService::init(tmp.path()).expect("init repo");
        svc.reindex().expect("initial reindex");
        svc.execute_sql("CREATE TABLE pair (a VARCHAR(64), b VARCHAR(64))")
            .expect("register pair typedef");
        let id = svc
            .create_doogat_with_extra("H5 typed row", &[], Some("pair"), "", BTreeMap::new())
            .expect("seed typed create")
            .meta
            .id
            .expect("created doogat must carry an id")
            .0;

        let lost = race_two_writers(&svc, &id);
        assert!(
            lost.is_empty(),
            "H5 fired (typed): a successful single-row column update vanished from HEAD, so a \
             concurrent typed update is whole-file last-writer-wins. Losing rounds:\n{}",
            lost.join("\n")
        );
    }

    /// Same race, but the typedef is SINGLETON so `needs_transaction` is true
    /// and the BEGIN IMMEDIATE (outer) → git lock (inner) path is what races.
    #[test]
    fn concurrent_singleton_typed_updates_on_one_doogat_keep_both_columns_or_refuse() {
        let tmp = tempfile::tempdir().unwrap();
        let mut svc = DoogatService::init(tmp.path()).expect("init repo");
        svc.reindex().expect("initial reindex");
        svc.execute_sql("CREATE TABLE cfg (a VARCHAR(64), b VARCHAR(64)) SINGLETON")
            .expect("register cfg singleton typedef");
        let id = svc
            .create_doogat_with_extra("H5 singleton row", &[], Some("cfg"), "", BTreeMap::new())
            .expect("seed singleton create")
            .meta
            .id
            .expect("created doogat must carry an id")
            .0;

        let lost = race_two_writers(&svc, &id);
        assert!(
            lost.is_empty(),
            "H5 fired (SINGLETON, transaction path): a successful single-row column update \
             vanished from HEAD, so a concurrent typed update under BEGIN IMMEDIATE is \
             whole-file last-writer-wins. Losing rounds:\n{}",
            lost.join("\n")
        );
    }

    /// The transaction decision is made before the git lock (SQLite-outer,
    /// git-inner). If a SINGLETON typedef for the update's result type lands
    /// between that preview and the locked read, the in-lock re-derive must
    /// refuse loudly instead of running the SINGLETON check outside the BEGIN
    /// IMMEDIATE window it needs. The updater is parked on the
    /// `AT_PREVIEW_TO_LOCK` seam (preview done, lock not yet taken); the test
    /// thread registers the typedef through the normal unlocked path, then
    /// releases it. Every wait is bounded, and a seam that never fires is a
    /// failure (the "paused" handshake times out), never a vacuous pass.
    #[test]
    fn refuses_when_a_singleton_typedef_for_the_result_type_lands_between_preview_and_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DoogatService::init(tmp.path()).expect("init repo");
        let id = svc
            .create_doogat("H5 retype target", &[], None, "")
            .expect("seed create");
        let path = format!("ddb/{id}.md");
        let before = svc.repo.read_file(&path).expect("read seed from HEAD");

        let (paused_tx, paused_rx) = mpsc::channel::<()>();
        let (resume_tx, resume_rx) = mpsc::channel::<()>();
        let updater_path = tmp.path().to_path_buf();
        let updater_id = id.clone();
        let updater = std::thread::spawn(move || {
            let svc = DoogatService::open(&updater_path).expect("open updater handle");
            AT_PREVIEW_TO_LOCK.set(Some(Box::new(move || {
                paused_tx.send(()).expect("test thread went away");
                resume_rx
                    .recv_timeout(ROUND_TIMEOUT)
                    .expect("never resumed from the preview-to-lock seam");
            })));
            // Preview runs with `app_config` unregistered: no transaction.
            svc.update_doogat(
                &updater_id,
                None,
                None,
                Some("app_config"),
                None,
                &ExtraFieldUpdates::default(),
            )
            .map_err(|e| e.to_string())
        });
        paused_rx
            .recv_timeout(ROUND_TIMEOUT)
            .expect("updater never reached the preview-to-lock seam");

        // Nobody holds the git lock now: register the SINGLETON typedef the
        // ordinary way (commit, index, materialize), then let the updater
        // take the lock and re-derive its requirement.
        let typedef = "---\nid: 20260510130000\ntitle: app_config\ntype: _typedef\n\
                       singleton: true\ncolumns:\n  - name: theme\n    data_type: TEXT\n\
                       \x20   zone: frontmatter\n---\n";
        let typedef_path = "ddb/_typedef/20260510130000.md";
        svc.repo
            .commit_file(typedef_path, typedef, "register singleton")
            .expect("commit typedef");
        svc.index
            .index_doogat(&parser::parse(typedef, typedef_path).expect("parse typedef"))
            .expect("index typedef");
        svc.index
            .materialize_all_types(&svc.repo)
            .expect("materialize typedef");
        resume_tx
            .send(())
            .expect("updater went away before resuming");

        let err = updater
            .join()
            .expect("updater thread panicked")
            .expect_err("retype into a typedef that became SINGLETON mid-update must refuse");
        assert!(
            err.contains("changed during the update"),
            "expected the concurrent-schema refusal, got: {err}"
        );
        assert_eq!(
            svc.repo.read_file(&path).expect("read HEAD after refusal"),
            before,
            "a refused update must leave the committed doogat untouched"
        );
    }

    /// The index must record THIS update's own commit as its indexed HEAD,
    /// not whatever HEAD is by the time `store_head` runs. The updater is
    /// parked on the `AT_COMMIT_TO_STORE_HEAD` seam (lock released, commit
    /// durable, index not yet touched); the test thread lands a later commit
    /// through its own handle, then releases the updater. Every wait is
    /// bounded; a seam that never fires times out instead of passing.
    #[test]
    fn stores_this_updates_own_commit_as_indexed_head_not_a_later_writers_head() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DoogatService::init(tmp.path()).expect("init repo");
        let id = svc
            .create_doogat("H5 attribution target", &[], None, "")
            .expect("seed create");

        let (paused_tx, paused_rx) = mpsc::channel::<()>();
        let (resume_tx, resume_rx) = mpsc::channel::<()>();
        let updater_path = tmp.path().to_path_buf();
        let updater_id = id.clone();
        let updater = std::thread::spawn(move || {
            let svc = DoogatService::open(&updater_path).expect("open updater handle");
            AT_COMMIT_TO_STORE_HEAD.set(Some(Box::new(move || {
                paused_tx.send(()).expect("test thread went away");
                resume_rx
                    .recv_timeout(ROUND_TIMEOUT)
                    .expect("never resumed from the commit-to-store-head seam");
            })));
            svc.update_doogat(
                &updater_id,
                Some("retitled"),
                None,
                None,
                None,
                &ExtraFieldUpdates::default(),
            )
            .map_err(|e| e.to_string())
        });
        paused_rx
            .recv_timeout(ROUND_TIMEOUT)
            .expect("updater never reached the commit-to-store-head seam");

        // The updater's commit is durable and the lock is released: HEAD is
        // its commit until the later writer moves it.
        let updaters_commit = svc
            .repo
            .head_oid()
            .expect("read HEAD after the update commit")
            .0;
        let later_commit = svc
            .repo
            .commit_file("ddb/other.md", "---\ntitle: other\n---\n", "later writer")
            .expect("later writer commit")
            .0;
        assert_ne!(updaters_commit, later_commit, "later writer must move HEAD");
        resume_tx
            .send(())
            .expect("updater went away before resuming");
        updater
            .join()
            .expect("updater thread panicked")
            .expect("update must succeed");

        let stored = svc.index.stored_head_oid();
        assert_eq!(
            stored.as_deref(),
            Some(updaters_commit.as_str()),
            "the index must store this update's own commit as its indexed HEAD"
        );
        assert_ne!(
            stored.as_deref(),
            Some(later_commit.as_str()),
            "the index must not attribute the update to the later writer's HEAD"
        );
    }
}
