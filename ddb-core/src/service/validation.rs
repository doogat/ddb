use crate::error::{DoogatError, Result};
use crate::traits::{GitBackend, IndexPort};
use crate::types::{BatchCreateInput, ParsedDoogat, TableSchema};

use super::DoogatService;

/// (table, column) -> current max value for bare DEFAULT NEXT columns.
pub(super) type BareNextCounters = std::collections::BTreeMap<(String, String), i64>;
/// (table, column, partition_value) -> current max value for DEFAULT NEXT(col) columns.
pub(super) type PartitionedNextCounters = std::collections::BTreeMap<(String, String, String), i64>;
/// A candidate unique_together key — `(table, group_columns, group_values)`.
/// FT-1: shared between the per-item DB check here and `batch_update`'s
/// intra-batch collision tracking (`update.rs`).
pub(super) type UniqueGroupKey = (String, Vec<String>, Vec<String>);

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Convert a `Value` to its canonical string representation for comparison
    /// against allowed_values and FK IDs. List/Map variants return None because
    /// they are not comparable to scalar constraints.
    pub(super) fn value_to_comparable_string(val: &crate::types::Value) -> Option<String> {
        match val {
            crate::types::Value::String(s) => Some(s.clone()),
            crate::types::Value::Number(n) => Some(n.to_string()),
            crate::types::Value::Bool(b) => {
                Some(if *b { "1".to_string() } else { "0".to_string() })
            }
            crate::types::Value::List(_) | crate::types::Value::Map(_) => None,
        }
    }

    /// Convert a `Value` to a string, always producing output.
    /// Unlike `value_to_comparable_string`, List/Map use debug format.
    pub(super) fn value_to_string(val: &crate::types::Value) -> String {
        match val {
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
        }
    }

    /// Check unique_together constraints for a single input.
    /// Returns `Ok(Some(parsed))` if a conflict was found and on_conflict is Ignore,
    /// `Ok(None)` if no conflict, or `Err` if on_conflict is Error.
    pub(super) fn check_unique_constraints(
        &self,
        input: &BatchCreateInput,
        schemas: &[TableSchema],
    ) -> Result<Option<ParsedDoogat>> {
        let type_name = match input.doogat_type {
            Some(ref t) => t,
            None => return Ok(None),
        };
        let schema = match schemas.iter().find(|s| s.table_name == *type_name) {
            Some(s) => s,
            None => return Ok(None),
        };
        let unique_groups = match schema.unique_together {
            Some(ref groups) => groups,
            None => return Ok(None),
        };

        for group in unique_groups {
            let (sql, param_vals) =
                match Self::build_unique_check_sql(type_name, group, &input.fields, None) {
                    Some(pair) => pair,
                    None => continue,
                };
            let rows = self.index.query_raw_with_params(&sql, &param_vals)?;
            let existing_id = match rows.first().and_then(|r| r.first()) {
                Some(id) => id,
                None => continue,
            };
            match input.on_conflict {
                crate::types::ConflictAction::Ignore => {
                    return Ok(Some(self.get_doogat_parsed(existing_id)?));
                }
                crate::types::ConflictAction::Error => {
                    let values: Vec<String> = group
                        .iter()
                        .map(|col| {
                            input
                                .fields
                                .get(col)
                                .map(Self::value_to_string)
                                .unwrap_or_default()
                        })
                        .collect();
                    return Err(DoogatError::unique_violation(
                        type_name.clone(),
                        group.clone(),
                        values,
                    ));
                }
            }
        }

        Ok(None)
    }

    /// PRD 00139 §3 layer 1: pre-INSERT singleton check. Mirrors the
    /// `check_unique_constraints` shape so callers see the same Ok/Err
    /// envelope regardless of which constraint blocked the row.
    ///
    /// - Returns `Ok(None)` when the typedef is not registered, not
    ///   singleton, or empty.
    /// - Returns `Ok(Some(existing))` when the typedef already holds a row
    ///   AND `on_conflict == Ignore` — caller treats this as an upsert
    ///   skip, identical to the unique-constraint Ignore branch.
    /// - Returns `Err(SINGLETON_VIOLATION)` otherwise.
    pub(super) fn check_singleton_constraint(
        &self,
        input: &BatchCreateInput,
        schemas: &[TableSchema],
    ) -> Result<Option<ParsedDoogat>> {
        let type_name = match input.doogat_type {
            Some(ref t) => t,
            None => return Ok(None),
        };
        let schema = match schemas.iter().find(|s| s.table_name == *type_name) {
            Some(s) => s,
            None => return Ok(None),
        };
        if !schema.singleton {
            return Ok(None);
        }
        // PRD 00139 cycle-3 #6: defensive identifier check before inlining
        // into the SQL string. Hyphenated typedef names are valid table
        // identifiers when quoted. Upstream typedef-name validation (DDL
        // parsing in sql_engine/ddl.rs + typedef YAML deserialization)
        // already restricts identifiers to this character class, so this
        // branch should be unreachable in practice. Fail loud in debug
        // builds and surface a structured validation error in release
        // builds rather than silently dropping enforcement (AGENTS.md
        // "no silent drops" guardrail).
        if !type_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            debug_assert!(
                false,
                "check_singleton_constraint reached with invalid typedef identifier {type_name:?}; upstream validation should have rejected this"
            );
            return Err(DoogatError::Validation(format!(
                "typedef name {type_name:?} contains characters outside [A-Za-z0-9_-]; singleton enforcement aborted"
            )));
        }
        // ORDER BY id keeps `existing_id` deterministic across Layer 1
        // (this check), Layer 2 (sql_engine/dml.rs SINGLETON pre-check), and
        // Layer 3 (`lookup_singleton_existing_id` after a UNIQUE-index hit).
        // Cycle-3 #5 pinned the ordering at `populate_materialized_table`;
        // pinning it at every consumer of that materialized table keeps the
        // invariant local rather than relying on SQLite's implicit rowid
        // order.
        let sql = format!("SELECT id FROM \"{type_name}\" ORDER BY id ASC LIMIT 1");
        let rows = self.index.query_raw_with_params(&sql, &[])?;
        let existing_id = match rows.first().and_then(|r| r.first()) {
            Some(id) => id,
            None => return Ok(None),
        };
        match input.on_conflict {
            crate::types::ConflictAction::Ignore => Ok(Some(self.get_doogat_parsed(existing_id)?)),
            crate::types::ConflictAction::Error => Err(DoogatError::singleton_violation(
                type_name.clone(),
                existing_id.clone(),
            )),
        }
    }

    /// Pre-UPDATE singleton check. Allows updating the existing singleton row
    /// in place, but rejects if the target typedef already has some *other*
    /// row materialized.
    pub(super) fn check_singleton_update_constraint(
        &self,
        current_id: &str,
        type_name: &str,
        schemas: &[TableSchema],
    ) -> Result<()> {
        let schema = match schemas.iter().find(|s| s.table_name == type_name) {
            Some(s) => s,
            None => return Ok(()),
        };
        if !schema.singleton {
            return Ok(());
        }
        // PRD 00139 cycle-3 #6: see check_singleton_constraint above for
        // the same identifier-shape rationale. Fail loud rather than
        // silently dropping enforcement.
        if !type_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            debug_assert!(
                false,
                "check_singleton_update_constraint reached with invalid typedef identifier {type_name:?}; upstream validation should have rejected this"
            );
            return Err(DoogatError::Validation(format!(
                "typedef name {type_name:?} contains characters outside [A-Za-z0-9_-]; singleton enforcement aborted"
            )));
        }
        let sql = format!("SELECT id FROM \"{type_name}\" WHERE id != ?1 LIMIT 1");
        let rows = self.index.query_raw_with_params(
            &sql,
            &[rusqlite::types::Value::Text(current_id.to_string())],
        )?;
        if let Some(existing_id) = rows.first().and_then(|r| r.first()) {
            return Err(DoogatError::singleton_violation(
                type_name.to_string(),
                existing_id.clone(),
            ));
        }
        Ok(())
    }

    /// Build the SQL query + params for checking one unique_together group.
    /// Returns `None` when not all group columns are present in the input fields.
    /// `exclude_id` (UPDATE lanes only) excludes the row being updated from
    /// the collision lookup, so a row may keep its own current value.
    fn build_unique_check_sql(
        type_name: &str,
        group: &[String],
        fields: &std::collections::BTreeMap<String, crate::types::Value>,
        exclude_id: Option<&str>,
    ) -> Option<(String, Vec<rusqlite::types::Value>)> {
        // PRD 00133: query the materialized typedef table directly. Before
        // unification, the service path joined `_ddb_fields` which only
        // indexes frontmatter; once typed creates routed TEXT columns to
        // body, the join missed those columns and unique_together silently
        // stopped catching duplicates. The materialized type-table writer
        // (`insert_materialized_row`) writes every column regardless of
        // zone, mirroring the SQL path's UNIQUE-index source of truth.
        let mut where_parts = Vec::with_capacity(group.len());
        let mut param_vals: Vec<rusqlite::types::Value> = Vec::with_capacity(group.len());

        for col_name in group.iter() {
            // Defensive identifier check; column names that aren't safe to
            // inline shouldn't reach this far, but reject rather than build
            // an injectable query. Hyphenated column names (like hyphenated
            // typedef names, allowed below) are valid identifiers once
            // quoted, so '-' is accepted alongside alphanumerics and '_'.
            if !col_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return None;
            }
            let val = fields.get(col_name)?;
            let val_str = Self::value_to_string(val);
            let val_idx = param_vals.len() + 1;
            param_vals.push(rusqlite::types::Value::Text(val_str));
            where_parts.push(format!("\"{col_name}\" = ?{val_idx}"));
        }

        // Hyphenated typedef names (e.g. `category-membership`) are valid as
        // table identifiers when quoted; allow them here.
        if !type_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        let mut sql = format!(
            "SELECT id FROM \"{type_name}\" WHERE {}",
            where_parts.join(" AND ")
        );
        if let Some(id) = exclude_id {
            let id_idx = param_vals.len() + 1;
            sql.push_str(&format!(" AND id != ?{id_idx}"));
            param_vals.push(rusqlite::types::Value::Text(id.to_string()));
        }
        sql.push_str(" LIMIT 1");
        Some((sql, param_vals))
    }

    /// Pre-UPDATE unique_together check (FT-1). Mirrors
    /// `check_unique_constraints`'s SELECT-against-materialized-table
    /// approach, but against the caller-supplied MERGED field set (existing
    /// values overlaid with the update's SET/UNSET) and excluding the row
    /// being updated, since a row may keep its own current value.
    ///
    /// Unlike the create-side check, an update always rejects a collision —
    /// `BatchUpdateInput` has no `on_conflict` — so this returns `Err` rather
    /// than `Ok(Some(existing))`.
    ///
    /// `merged_fields` alone may be missing a group column the update didn't
    /// touch (a Body-zone column lives only in `parsed.body`, never in
    /// `meta.extra`/`inline_fields`); `merge_row_snapshot_for_unique_check`
    /// folds in that row's own materialized value for any such column before
    /// the group is checked, instead of silently skipping the whole group.
    pub(super) fn check_unique_constraints_for_update(
        &self,
        current_id: &str,
        type_name: &str,
        merged_fields: &std::collections::BTreeMap<String, crate::types::Value>,
        schemas: &[TableSchema],
    ) -> Result<()> {
        let schema = match schemas.iter().find(|s| s.table_name == type_name) {
            Some(s) => s,
            None => return Ok(()),
        };
        if schema.unique_together.is_none() {
            return Ok(());
        }
        let effective_fields =
            self.merge_row_snapshot_for_unique_check(current_id, type_name, merged_fields, schemas)?;
        self.check_effective_unique_fields(current_id, type_name, schema, &effective_fields)
    }

    /// FT-1 rework: fold the row's own materialized value into
    /// `merged_fields` for any unique_together column the update's SET/UNSET
    /// didn't mention. Without this, a group spanning a column outside
    /// `meta.extra`/`inline_fields` (e.g. a Body-zone typed column) is
    /// missing from `merged_fields`, `build_unique_check_sql` returns `None`
    /// for it (`fields.get(col)?`), and the whole group is skipped even
    /// though materialization writes every column regardless of zone.
    /// Returns `merged_fields` unchanged when `type_name` isn't registered,
    /// has no unique_together groups, or every group column is already
    /// present.
    pub(super) fn merge_row_snapshot_for_unique_check(
        &self,
        current_id: &str,
        type_name: &str,
        merged_fields: &std::collections::BTreeMap<String, crate::types::Value>,
        schemas: &[TableSchema],
    ) -> Result<std::collections::BTreeMap<String, crate::types::Value>> {
        let mut effective = merged_fields.clone();
        let schema = match schemas.iter().find(|s| s.table_name == type_name) {
            Some(s) => s,
            None => return Ok(effective),
        };
        let groups = match schema.unique_together.as_ref() {
            Some(g) => g,
            None => return Ok(effective),
        };

        let mut missing: Vec<&str> = groups
            .iter()
            .flatten()
            .map(|c| c.as_str())
            .filter(|c| !merged_fields.contains_key(*c))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        if missing.is_empty() {
            return Ok(effective);
        }

        // Defensive identifier check mirrors `build_unique_check_sql`; leave
        // the affected group(s) to be skipped there rather than build an
        // injectable query. Hyphens are accepted alongside alphanumerics and
        // '_' so a hyphenated unique_together column isn't silently dropped
        // from enforcement.
        let identifiers_safe = missing
            .iter()
            .all(|c| c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
            && type_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !identifiers_safe {
            return Ok(effective);
        }

        let select_cols = missing
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT {select_cols} FROM \"{type_name}\" WHERE id = ?1");
        let rows = self.index.query_raw_with_params(
            &sql,
            &[rusqlite::types::Value::Text(current_id.to_string())],
        )?;
        if let Some(row) = rows.first() {
            for (col, val) in missing.iter().zip(row.iter()) {
                effective.insert((*col).to_string(), crate::types::Value::String(val.clone()));
            }
        }
        Ok(effective)
    }

    /// Run the unique_together group checks against an already-resolved
    /// effective field set (see `merge_row_snapshot_for_unique_check`).
    /// Split out from `check_unique_constraints_for_update` so the batch
    /// lane (`prepare_update`) can reuse the same effective-field snapshot
    /// both for this per-item DB check and for the intra-batch collision
    /// keys (`unique_group_candidates`), without computing it twice.
    pub(super) fn check_effective_unique_fields(
        &self,
        current_id: &str,
        type_name: &str,
        schema: &TableSchema,
        effective_fields: &std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<()> {
        let unique_groups = match schema.unique_together.as_ref() {
            Some(groups) => groups,
            None => return Ok(()),
        };

        for group in unique_groups {
            let (sql, param_vals) = match Self::build_unique_check_sql(
                type_name,
                group,
                effective_fields,
                Some(current_id),
            ) {
                Some(pair) => pair,
                None => continue,
            };
            let rows = self.index.query_raw_with_params(&sql, &param_vals)?;
            if rows.first().and_then(|r| r.first()).is_some() {
                let values: Vec<String> = group
                    .iter()
                    .map(|col| {
                        effective_fields
                            .get(col)
                            .map(Self::value_to_string)
                            .unwrap_or_default()
                    })
                    .collect();
                return Err(DoogatError::unique_violation(
                    type_name.to_string(),
                    group.clone(),
                    values,
                ));
            }
        }

        Ok(())
    }

    /// FT-1: candidate `(type, group_columns, values)` keys for every
    /// fully-resolvable unique_together group under `effective_fields`.
    /// `batch_update`'s Phase 1 uses this to catch two updates in the same
    /// batch that would land on the same UNIQUE tuple only after commit —
    /// each passes `check_effective_unique_fields` alone (neither row is
    /// materialized yet), so the per-item DB check can't see the collision.
    /// Mirrors `batch.rs`'s `input_unique_keys` for creates.
    pub(super) fn unique_group_candidates(
        type_name: &str,
        schema: &TableSchema,
        effective_fields: &std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Vec<UniqueGroupKey> {
        let groups = match schema.unique_together.as_ref() {
            Some(g) => g,
            None => return vec![],
        };
        groups
            .iter()
            .filter_map(|group| {
                let values: Option<Vec<String>> = group
                    .iter()
                    .map(|col| effective_fields.get(col).map(Self::value_to_string))
                    .collect();
                values.map(|vals| (type_name.to_string(), group.clone(), vals))
            })
            .collect()
    }

    /// Validate extra fields against a pre-loaded typedef schema list.
    /// Callers load schemas once per operation to avoid redundant queries.
    pub(super) fn validate_fields_with_schemas(
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
            // Reject structured values on declared scalar columns. CREATE rejects
            // these via `stringify_typed_input_fields`; UPDATE must match that behavior.
            if let Some(val) = extra.get(&col.name) {
                if matches!(
                    val,
                    crate::types::Value::List(_) | crate::types::Value::Map(_)
                ) {
                    return Err(DoogatError::Validation(format!(
                        "field '{}' has structured value (list/map) but typed column '{}.{}' expects a scalar",
                        col.name, type_name, col.name
                    )));
                }
            }
            Self::validate_allowed_values_comparable(col, extra)?;
            self.validate_fk_reference_comparable(col, extra)?;
        }
        Ok(())
    }

    /// Validate allowed_values using comparable (scalar-only) string conversion.
    fn validate_allowed_values_comparable(
        col: &crate::types::ColumnDef,
        extra: &std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<()> {
        let allowed = match col.allowed_values {
            Some(ref a) => a,
            None => return Ok(()),
        };
        let val = match extra.get(&col.name) {
            Some(v) => v,
            None => return Ok(()),
        };
        let val_str = match Self::value_to_comparable_string(val) {
            Some(s) => s,
            None => return Ok(()), // structured values can't match scalar allowed_values
        };
        if !allowed.contains(&val_str) {
            return Err(DoogatError::Validation(format!(
                "field '{}' value '{}' not in allowed values: {:?}",
                col.name, val_str, allowed
            )));
        }
        Ok(())
    }

    /// Validate FK reference using comparable (scalar-only) string conversion.
    fn validate_fk_reference_comparable(
        &self,
        col: &crate::types::ColumnDef,
        extra: &std::collections::BTreeMap<String, crate::types::Value>,
    ) -> Result<()> {
        let ref_table = match col.references {
            Some(ref r) => r,
            None => return Ok(()),
        };
        let val = match extra.get(&col.name) {
            Some(v) => v,
            None => return Ok(()),
        };
        let val_str = match Self::value_to_comparable_string(val) {
            Some(s) => s,
            None => return Ok(()), // structured values can't be FK IDs
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
        Ok(())
    }
}
