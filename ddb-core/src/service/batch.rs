use crate::error::{DoogatError, Result};
use crate::git_ops;
use crate::parser;
use crate::types::{BatchCreateInput, DoogatId, DoogatMeta, ParsedDoogat, TableSchema};

use crate::traits::{GitBackend, IndexPort};

use super::validation::{BareNextCounters, PartitionedNextCounters};
use super::DoogatService;

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

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
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

        // PRD 00140: when any input targets a registered SINGLETON
        // typedef, run Phases 1+2+3 inside a `BEGIN IMMEDIATE` transaction so
        // a cross-process race surfaces a structured SINGLETON_VIOLATION on
        // the losing writer instead of a raw SQL error.
        let any_singleton = inputs
            .iter()
            .any(|input| singleton_type_for(input, &schemas).is_some());

        let mut body = || -> Result<Vec<ParsedDoogat>> {
            self.batch_create_body(
                inputs,
                message,
                &schemas,
                &mut next_counters,
                &mut partitioned_counters,
            )
        };

        if any_singleton {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), body)
        } else {
            body()
        }
    }

    /// Phases 1-3 of `batch_create_with_message`, factored out so the
    /// SINGLETON-typedef path can wrap them in a `BEGIN IMMEDIATE`
    /// transaction (PRD 00139 §4).
    fn batch_create_body(
        &self,
        inputs: &[BatchCreateInput],
        message: &str,
        schemas: &[TableSchema],
        next_counters: &mut BareNextCounters,
        partitioned_counters: &mut PartitionedNextCounters,
    ) -> Result<Vec<ParsedDoogat>> {
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
            if let Some(existing) = self.check_singleton_constraint(input, schemas)? {
                results[input_idx] = Some(existing);
                continue;
            }

            if let Some(existing) = self.check_unique_constraints(input, schemas)? {
                results[input_idx] = Some(existing);
                continue;
            }

            // PRD 00139 §4: intra-batch SINGLETON tracker. A second
            // SINGLETON insert into the same typedef in this batch must
            // reject (Error) or skip-to-survivor (Ignore) before commit.
            if let Some(type_name) = singleton_type_for(input, schemas) {
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
                find_intra_batch_duplicate(input, schemas, &seen_unique)
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

            record_intra_batch_unique(input, schemas, &mut seen_unique, input_idx);

            let (path, content) =
                self.prepare_create(input, schemas, next_counters, partitioned_counters)?;
            writes.push((input_idx, path, content));
        }

        // Phase 2: atomic commit
        self.commit_batch_creates(&writes, message)?;

        // Phase 3: index new writes and fill result slots
        self.index_batch_creates(&writes, schemas, &mut results)?;

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

    /// Phase 2: atomic commit for batch creates (no-op when writes is empty).
    fn commit_batch_creates(
        &self,
        writes: &[(usize, String, String)],
        message: &str,
    ) -> Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let write_refs: Vec<(&str, &str)> = writes
            .iter()
            .map(|(_, p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_batch(&write_refs, &[], message)?;
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
}
