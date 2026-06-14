use crate::error::{DoogatError, Result};
use crate::parser;
use crate::sql_engine::build_data_doogat;
use crate::sql_engine::typed_insert::{prepare_typed_insert_validate, TypedInsertCounters};
use crate::types::{BatchCreateInput, DoogatId, ParsedDoogat, TableSchema};

use crate::traits::{GitBackend, IndexPort};

use super::validation::{BareNextCounters, PartitionedNextCounters};
use super::DoogatService;

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

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Re-parse content, index, dual-write, and rematerialize the type table row.
    pub(super) fn reindex_and_rematerialize(
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

    /// Typed-create branch of `prepare_create`. Stringifies input fields,
    /// delegates default + ENUM + FK validation to
    /// `prepare_typed_insert_validate`, enforces post-defaults NOT NULL, and
    /// builds the `ParsedDoogat` via `build_data_doogat` so REFERENCES
    /// values land in the reference zone.
    pub(super) fn build_typed_create(
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
            self.index.sql_conn(),
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
            Some(self.index.sql_conn()),
        );
        parsed.meta.tags = input.tags.clone();
        if let Some(ref body) = input.body {
            parsed.body = body.clone();
        }
        parsed.path = path.to_string();
        Ok(parsed)
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
    pub(super) fn resolve_create_title(
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
                Some(self.index.sql_conn()),
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

    /// PRD 00129 §1: pre-defaults validation for typed creates. Runs
    /// before `resolve_column_defaults` so unknown fields and unregistered
    /// types reject without polluting the defaults pipeline. NOT NULL
    /// runs in [`validate_typed_create_post_defaults`] because a column
    /// default can legitimately satisfy a NOT NULL column.
    pub(super) fn validate_typed_create_pre_defaults(
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
    pub(super) fn validate_typed_create_post_defaults(
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

    /// Generate a unique doogat ID, checking the filesystem for collisions.
    pub(super) fn unique_id(&self) -> DoogatId {
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

    /// Best-effort dual-write to the NoSQL mirror via the injected port.
    /// Mirror failures are swallowed (silent best-effort), preserving the prior
    /// behavior; the production Redb mirror opens the database per call and the
    /// no-op mirror does nothing when the `nosql` feature is disabled.
    pub(super) fn nosql_index_doogat(&self, doogat: &ParsedDoogat) {
        let _ = self.nosql.mirror_index_doogat(doogat);
    }
}

/// Stringify a typed `BatchCreateInput`'s fields into the `BTreeMap<String, String>`
/// shape `prepare_typed_insert_validate` expects. Reserved core columns
/// (id/title/type/date/created_at/updated_at/tags) are skipped — they don't
/// participate in zone routing or per-column validation. Structured values
/// (`List`/`Map`) on declared scalar columns reject up front; they cannot
/// satisfy `allowed_values` or FK checks and would silently disappear in
/// `value_to_comparable_string`.
pub(super) fn stringify_typed_input_fields(
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
pub(super) fn extract_helper_counters_for_table(
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
pub(super) fn write_back_helper_counters(
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
