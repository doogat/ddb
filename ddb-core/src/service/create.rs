use crate::app_contract::{AppOutput, AppWarning, CreateCommand, UnregisteredTypePolicy};
use crate::error::{DoogatError, Result};
use crate::git_ops;
use crate::parser;
use crate::sql_engine::build_data_doogat;
use crate::sql_engine::typed_insert::{prepare_typed_insert_validate, TypedInsertCounters};
use crate::types::{BatchCreateInput, DoogatId, DoogatMeta, ParsedDoogat, TableSchema};

use crate::traits::{GitBackend, IndexPort};

use super::update::parsed_fields;
use super::write_helpers::stringify_typed_input_fields;
use super::{DoogatService, ExtraFieldUpdates};

/// Outcome of `DoogatService::upsert_singleton`: the affected row id and
/// whether the row was newly created (`true`) or an existing row was
/// updated in place (`false`).
#[derive(Debug)]
pub struct UpsertOutcome {
    pub id: String,
    pub created: bool,
}

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
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

    /// App facade entrypoint: create a doogat from a `CreateCommand`.
    /// Routes through `batch_create_with_message` for full behavior parity:
    /// title_template rendering, SINGLETON Ignore semantics, NOT_NULL_VIOLATION,
    /// TYPE_NOT_REGISTERED, and field validation.
    pub fn create(&self, cmd: CreateCommand) -> Result<AppOutput<ParsedDoogat>> {
        if let Some(output) = self.try_baseonly_unregistered(&cmd)? {
            return Ok(output);
        }

        let caller_title = cmd.title.clone();
        let doogat_type = cmd.doogat_type.clone();
        let input = crate::types::BatchCreateInput {
            title: cmd.title,
            body: cmd.body,
            tags: cmd.tags,
            doogat_type: cmd.doogat_type,
            fields: cmd.fields,
            on_conflict: cmd.on_conflict,
        };
        let mut results = self.batch_create_with_message(&[input], "create doogat")?;
        let value = results
            .pop()
            .ok_or_else(|| DoogatError::Validation("batch_create returned empty".into()))?;

        let warnings = self
            .title_from_template_warning(caller_title.is_none(), doogat_type.as_deref(), &value)
            .into_iter()
            .collect();

        Ok(AppOutput { value, warnings })
    }

    /// PRD 00155: if the caller uses `BaseOnly` policy and the requested type
    /// is not a registered typedef, fall back to a base doogat and return a
    /// warning rather than rejecting. Returns `Ok(None)` when the policy is
    /// `Strict`, no type was specified, or the type IS registered (fall
    /// through to the typed pipeline).
    ///
    /// Note: `on_conflict` is not honored on this path — `create_doogat_with_extra`
    /// hardcodes `ConflictAction::Error`. The only BaseOnly caller is the CLI,
    /// which also uses `Error`, so this is currently a non-issue.
    fn try_baseonly_unregistered(
        &self,
        cmd: &CreateCommand,
    ) -> Result<Option<AppOutput<ParsedDoogat>>> {
        if cmd.unregistered_type_policy != UnregisteredTypePolicy::BaseOnly {
            return Ok(None);
        }
        let ty = match cmd.doogat_type.as_deref() {
            Some(ty) => ty,
            None => return Ok(None),
        };
        let schemas = self.list_type_schemas()?;
        if schemas.iter().any(|s| s.table_name == ty) {
            return Ok(None);
        }
        let value = self.create_doogat_with_extra(
            cmd.title.as_deref().unwrap_or(""),
            &cmd.tags,
            Some(ty),
            cmd.body.as_deref().unwrap_or(""),
            cmd.fields.clone(),
        )?;
        Ok(Some(AppOutput {
            value,
            warnings: vec![AppWarning {
                code: "UNREGISTERED_TYPE_BASE_ONLY",
                message: format!(
                    "type '{ty}' is not a registered typedef; created a base doogat without typed validation"
                ),
            }],
        }))
    }

    /// Emit `TITLE_FROM_TEMPLATE` only when the caller omitted a title AND the
    /// requested type has a `title_template` declared on its typedef.
    /// Binding to `title_template` (rather than "result has a non-empty
    /// title") narrows the heuristic so future auto-title mechanisms don't
    /// trigger this code, and it makes the warning's message ("title was
    /// rendered from typedef title_template") accurate by construction for
    /// the common path. The `on_conflict=Ignore` skip path is a known
    /// residual false positive: when an existing row is returned unchanged
    /// the warning still fires even though no rendering happened on this
    /// call. Eliminating that case requires plumbing a "title rendered"
    /// signal out of `batch_create` and is left for a follow-up.
    fn title_from_template_warning(
        &self,
        caller_title_is_none: bool,
        doogat_type: Option<&str>,
        value: &ParsedDoogat,
    ) -> Option<AppWarning> {
        if !caller_title_is_none {
            return None;
        }
        let ty = doogat_type?;
        let schemas = self.index.load_all_typedefs(&self.repo);
        let template_present = schemas
            .get(ty)
            .and_then(|s| s.title_template.as_ref())
            .is_some();
        if !template_present {
            return None;
        }
        let title_nonempty = value.meta.title.as_deref().is_some_and(|t| !t.is_empty());
        if title_nonempty {
            Some(AppWarning {
                code: "TITLE_FROM_TEMPLATE",
                message: "title was rendered from typedef title_template".into(),
            })
        } else {
            None
        }
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
        let id = self.unique_id()?;
        let id_str = id.to_string();

        let folder = doogat_type
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let path = git_ops::doogat_path(&id_str, doogat_type, folder);

        let schemas = self.list_type_schemas()?;
        let typedef = match doogat_type {
            Some(t) => schemas.iter().find(|s| s.table_name == t).cloned(),
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

        // PRD 00140: when the type is a registered SINGLETON typedef, run
        // the SINGLETON pre-check → commit → index window inside a
        // `BEGIN IMMEDIATE` transaction so a cross-process race surfaces a
        // structured SINGLETON_VIOLATION on the losing writer.
        let is_singleton = typedef.as_ref().map(|s| s.singleton).unwrap_or(false);

        let write = || -> Result<ParsedDoogat> {
            let _ = self.check_singleton_constraint(&synth_input, &schemas)?;

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
        };

        // The IMMEDIATE write lock is held across the git `commit_file` inside
        // `write` — deliberate (PRD 00140): a SINGLETON commit writes one small
        // markdown file, so the hold is sub-millisecond against the 5000ms
        // `busy_timeout`. If `index_doogat`/`materialize_single` fail after the
        // commit, the SQLite side rolls back but the git commit stays; the next
        // `ensure_fresh` detects the advanced HEAD and re-indexes, and
        // `consistency::singleton_sweep` (PRD 00139 §10) reconciles any duplicate.
        if is_singleton {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), write)
        } else {
            write()
        }
    }

    /// Create-or-update the single row of a SINGLETON typedef.
    ///
    /// PRD 00140 (Approach C): the existing-row check and the create-or-update
    /// run under one `ensure_fresh()` + `BEGIN IMMEDIATE` window so concurrent
    /// upserts on a SINGLETON typedef converge on one row. Rejects
    /// non-SINGLETON typedefs — this method is SINGLETON-only.
    pub fn upsert_singleton(
        &self,
        type_name: &str,
        fields: std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<UpsertOutcome> {
        // Refresh BEFORE opening the transaction so the IMMEDIATE write lock is
        // not held across a potentially-long reindex, keeping the lock window
        // tight in the common (already-fresh) case. The authoritative refresh
        // for cross-process convergence happens in-lock below.
        self.ensure_fresh()?;

        let schemas = self.list_type_schemas()?;
        let is_singleton = schemas
            .iter()
            .find(|s| s.table_name == type_name)
            .map(|s| s.singleton)
            .unwrap_or(false);
        if !is_singleton {
            return Err(DoogatError::Validation(format!(
                "{type_name} is not a SINGLETON typedef"
            )));
        }

        crate::indexer::with_immediate_transaction(self.index.sql_conn(), || {
            // Reconcile freshness INSIDE the IMMEDIATE window (PRD 00157) so the
            // loser of a cross-process race observes the winner's just-committed
            // row before the SELECT below decides create-vs-update. The loser
            // only acquires this lock after the winner's COMMIT released it, so
            // by now the winner's row is fully materialized and `_ddb_meta.head`
            // has advanced; without this in-lock refresh the loser's pre-lock
            // refresh may predate that commit, leaving its SELECT stale → a
            // second CREATE → a duplicate SINGLETON row. Inside a transaction
            // `rebuild_if_stale` performs only the nesting-safe incremental
            // reindex (never a destructive full rebuild — see its docs), so it
            // composes with the open `BEGIN IMMEDIATE`. Usually a no-op once
            // `_ddb_meta.head` already equals repo HEAD.
            self.ensure_fresh()?;
            let escaped = type_name.replace('"', "\"\"");
            let sql = format!("SELECT id FROM \"{escaped}\" ORDER BY id ASC LIMIT 1");
            let rows = self.index.query_raw_with_params(&sql, &[])?;
            let existing_id = rows.first().and_then(|r| r.first()).cloned();

            match existing_id {
                Some(existing_id) => {
                    let extra = ExtraFieldUpdates {
                        set: &fields,
                        unset: &[],
                    };
                    self.update_doogat(&existing_id, None, None, None, None, &extra)?;
                    Ok(UpsertOutcome {
                        id: existing_id,
                        created: false,
                    })
                }
                None => {
                    let parsed =
                        self.create_doogat_with_extra(type_name, &[], Some(type_name), "", fields)?;
                    let id = parsed.meta.id.map(|z| z.0).unwrap_or_default();
                    Ok(UpsertOutcome { id, created: true })
                }
            }
        })
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

        prepare_typed_insert_validate(
            schema,
            &mut col_values,
            &mut counters,
            self.index.sql_conn(),
        )?;

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
            Some(self.index.sql_conn()),
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
    ///
    /// PRD 00139 cycle-3 task #1: raw-frontmatter semantics. The user's
    /// markdown is written to git verbatim (modulo path computation) so
    /// arbitrary frontmatter keys round-trip and unregistered `type:` values
    /// stay as authored. SINGLETON and UNIQUE constraints still fire when
    /// the type IS a registered typedef — the raw path runs
    /// `check_singleton_constraint` and `check_unique_constraints` (same
    /// helpers `batch_create` uses) before any commit. Unknown types are
    /// accepted (legacy FFI contract): authors can write `type: foo` even
    /// when `foo` has no typedef, matching pre-cycle-1 behavior.
    pub fn create_doogat_raw(&self, content: &str, message: &str) -> Result<String> {
        self.ensure_fresh()?;
        let parsed = parser::parse(content, "new.md")?;
        let id = match parsed.meta.id.as_ref() {
            Some(z) => z.0.clone(),
            None => self.unique_id()?.0,
        };

        let folder = parsed
            .meta
            .doogat_type
            .as_deref()
            .map(|t| self.index.type_uses_folder(t, &self.repo))
            .unwrap_or(false);
        let rel_path = git_ops::doogat_path(&id, parsed.meta.doogat_type.as_deref(), folder);
        let schemas = self.list_type_schemas()?;

        // PRD 00140: a write into a registered SINGLETON typedef runs its
        // constraint-check → commit → index window inside a `BEGIN IMMEDIATE`
        // transaction so a cross-process race surfaces a structured
        // SINGLETON_VIOLATION on the losing writer, not a raw SQL error.
        let is_singleton = parsed
            .meta
            .doogat_type
            .as_deref()
            .and_then(|t| schemas.iter().find(|s| s.table_name == t))
            .map(|s| s.singleton)
            .unwrap_or(false);

        let write = || -> Result<()> {
            // Run SINGLETON + UNIQUE + FK / allowed_values checks only when
            // the declared type is registered. Unregistered types skip
            // validation (raw FFI contract).
            if let Some(type_name) = parsed.meta.doogat_type.as_deref() {
                if let Some(schema) = schemas.iter().find(|s| s.table_name == type_name) {
                    let input = build_batch_create_from_parsed(&parsed);
                    let _ = self.check_singleton_constraint(&input, &schemas)?;
                    let _ = self.check_unique_constraints(&input, &schemas)?;
                    // Raw FFI accepts frontmatter values for typedef columns
                    // regardless of declared zone, so a column the typedef
                    // marks Body-zone (e.g. plain `TEXT UNIQUE`) still has its
                    // value in `meta.extra`. The materialized typed table is
                    // populated zone-strictly, so `check_unique_constraints`
                    // misses cross-row duplicates whose values landed in
                    // `_ddb_fields` instead. Fall back to that index here.
                    self.check_unique_via_ddb_fields(&parsed, schema)?;
                    // PRD 00134 batch-end follow-up: parity with the typed
                    // create path for FK + allowed_values. Without this, raw
                    // FFI lets dangling REFERENCES through entirely (no Layer
                    // 3 enforcement on FK in materialized tables) and surfaces
                    // ENUM violations as opaque `DoogatError::Sql("CHECK
                    // constraint failed: ...")` instead of a friendly
                    // validation message.
                    self.validate_fields_with_schemas(&schemas, type_name, &input.fields)?;
                }
            }

            // Write the user's bytes verbatim — no re-serialize through the
            // typed-write pipeline, which would drop top-level meta fields
            // (`date`) and arbitrary custom keys.
            self.repo.commit_file(&rel_path, content, message)?;
            let parsed = parser::parse(content, &rel_path)?;
            self.index.index_doogat(&parsed)?;
            // PRD 00134 blind-review I2: when raw Markdown carries a registered
            // typedef, populate the typed table + auto-junctions alongside the
            // metadata index update so SQLite-level UNIQUE indexes also catch
            // late conflicts the pre-check couldn't see.
            if let Some(type_name) = parsed.meta.doogat_type.as_deref() {
                if let Some(schema) = schemas.iter().find(|s| s.table_name == type_name) {
                    self.index.materialize_single(schema, &id, &parsed)?;
                }
            }
            self.nosql_index_doogat(&parsed);
            Ok(())
        };

        if is_singleton {
            crate::indexer::with_immediate_transaction(self.index.sql_conn(), write)?;
        } else {
            write()?;
        }

        Ok(id)
    }

    /// PRD 00139 cycle-3 task #1: raw-FFI UNIQUE fallback against
    /// `_ddb_fields`. `check_unique_constraints` queries the materialized
    /// typed table, which is populated zone-strictly — a typedef-declared
    /// Body-zone column stays NULL in the typed table when the author put
    /// the value in frontmatter via raw markdown. `_ddb_fields` indexes
    /// every frontmatter value with its `doogat_id`, so a same-typed
    /// duplicate is detectable here.
    ///
    /// Single-column UNIQUE groups (`unique_together: [["url"]]`) are the
    /// common shape from `CREATE TABLE x (col TEXT UNIQUE)`. Multi-column
    /// groups still work as long as every column value is in
    /// `_ddb_fields` (i.e. authored in frontmatter); rows that split a
    /// composite key across zones fall through to the materialized-table
    /// check above.
    fn check_unique_via_ddb_fields(
        &self,
        parsed: &ParsedDoogat,
        schema: &TableSchema,
    ) -> Result<()> {
        let groups = match schema.unique_together.as_ref() {
            Some(g) => g,
            None => return Ok(()),
        };
        let new_id = parsed
            .meta
            .id
            .as_ref()
            .map(|z| z.0.as_str())
            .unwrap_or_default();
        for group in groups {
            let values: Option<Vec<String>> = group
                .iter()
                .map(|col| {
                    parsed
                        .meta
                        .extra
                        .get(col)
                        .and_then(Self::value_to_comparable_string)
                })
                .collect();
            let values = match values {
                Some(v) => v,
                None => continue,
            };

            // Find any same-type row that has all these (key, value) pairs
            // recorded in `_ddb_fields`. The query intersects the matching
            // doogat_id sets across the group columns and filters out the
            // row we're about to write.
            let mut sql =
                String::from("SELECT z.id FROM doogats z WHERE z.type = ?1 AND z.id != ?2");
            let mut params: Vec<rusqlite::types::Value> = vec![
                rusqlite::types::Value::Text(schema.table_name.clone()),
                rusqlite::types::Value::Text(new_id.to_string()),
            ];
            for (col, value) in group.iter().zip(values.iter()) {
                let key_idx = params.len() + 1;
                let val_idx = params.len() + 2;
                sql.push_str(&format!(
                    " AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{key_idx} AND value = ?{val_idx})"
                ));
                params.push(rusqlite::types::Value::Text(col.clone()));
                params.push(rusqlite::types::Value::Text(value.clone()));
            }
            sql.push_str(" LIMIT 1");
            let rows = self.index.query_raw_with_params(&sql, &params)?;
            if rows.first().and_then(|r| r.first()).is_some() {
                return Err(DoogatError::unique_violation(
                    schema.table_name.clone(),
                    group.clone(),
                    values,
                ));
            }
        }
        Ok(())
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
