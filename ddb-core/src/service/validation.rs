use crate::error::{DoogatError, Result};
use crate::traits::GitBackend;
use crate::types::{BatchCreateInput, ParsedDoogat, TableSchema};

use super::DoogatService;

/// (table, column) -> current max value for bare DEFAULT NEXT columns.
pub(super) type BareNextCounters = std::collections::BTreeMap<(String, String), i64>;
/// (table, column, partition_value) -> current max value for DEFAULT NEXT(col) columns.
pub(super) type PartitionedNextCounters = std::collections::BTreeMap<(String, String, String), i64>;

impl<G: GitBackend> DoogatService<G> {
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
                match Self::build_unique_check_sql(type_name, group, &input.fields) {
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
        // Defensive identifier check before inlining into the SQL string.
        // Hyphenated typedef names are valid table identifiers when quoted.
        if !type_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Ok(None);
        }
        let sql = format!("SELECT id FROM \"{type_name}\" LIMIT 1");
        let rows = self.index.query_raw_with_params(&sql, &[])?;
        let existing_id = match rows.first().and_then(|r| r.first()) {
            Some(id) => id,
            None => return Ok(None),
        };
        match input.on_conflict {
            crate::types::ConflictAction::Ignore => {
                Ok(Some(self.get_doogat_parsed(existing_id)?))
            }
            crate::types::ConflictAction::Error => Err(DoogatError::singleton_violation(
                type_name.clone(),
                existing_id.clone(),
            )),
        }
    }

    /// Build the SQL query + params for checking one unique_together group.
    /// Returns `None` when not all group columns are present in the input fields.
    fn build_unique_check_sql(
        type_name: &str,
        group: &[String],
        fields: &std::collections::BTreeMap<String, crate::types::Value>,
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
            // an injectable query.
            if !col_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        let sql = format!(
            "SELECT id FROM \"{type_name}\" WHERE {} LIMIT 1",
            where_parts.join(" AND ")
        );
        Some((sql, param_vals))
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
