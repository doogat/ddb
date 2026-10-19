use crate::error::{DoogatError, Result};
use crate::types::{TableSchema, Zone};

use crate::sql_engine::{SqlEngine, SqlResult};

impl<'a> SqlEngine<'a> {
    pub(in crate::sql_engine) fn handle_set_zone(
        &mut self,
        table_name: &str,
        zone_str: &str,
        column_name: &str,
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        let col_lower = column_name.to_lowercase();
        let col = schema
            .columns
            .iter_mut()
            .find(|c| c.name == col_lower)
            .ok_or_else(|| DoogatError::SqlEngine(format!("column not found: {column_name}")))?;
        let zone = match zone_str.to_lowercase().as_str() {
            "frontmatter" => Zone::Frontmatter,
            "body" => Zone::Body,
            "reference" => Zone::Reference,
            _ => {
                return Err(DoogatError::SqlEngine(format!(
                    "invalid zone: {zone_str} (use frontmatter, body, or reference)"
                )))
            }
        };
        col.zone = Some(zone);
        self.update_typedef(table_name, &schema)?;
        self.rematerialize_or_defer(table_name)?;
        Ok(SqlResult::Ok(format!(
            "zone set to {zone_str} for {col_lower} in {table_name}"
        )))
    }

    pub(in crate::sql_engine) fn handle_title_template(
        &mut self,
        table_name: &str,
        template: Option<&str>,
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        if let Some(tmpl) = template {
            self.validate_title_template(&schema, tmpl)?;
        }
        schema.title_template = template.map(String::from);
        self.update_typedef(table_name, &schema)?;
        let action = if template.is_some() { "set" } else { "dropped" };
        Ok(SqlResult::Ok(format!(
            "title template {action} for {table_name}"
        )))
    }

    /// `ALTER TABLE <name> SET SEARCH KEY <col>` (and DROP form). Tells the
    /// search filter resolver to match `<col>=val` substring queries against
    /// `<col>` on the typedef table instead of the default `title`. Useful
    /// when the canonical user-facing identifier of a typedef is something
    /// other than its title (e.g. jink categories key off `fqn`).
    /// Validates that the column exists on the typedef and is not a
    /// REFERENCES column (search keys must be human-readable).
    pub(in crate::sql_engine) fn handle_search_key(
        &mut self,
        table_name: &str,
        column: Option<&str>,
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        if let Some(col_name) = column {
            let col_lower = col_name.to_lowercase();
            // `id`, `title`, `date`, `updated_at`, `type` count as core columns.
            // We allow `title` (it is the default but explicit is fine), but
            // reject `id` / `type` / `date` / `updated_at` because they're
            // either not user-facing or already matched via the core path.
            if col_lower != "title" {
                let col = schema
                    .columns
                    .iter()
                    .find(|c| c.name == col_lower)
                    .ok_or_else(|| {
                        DoogatError::SqlEngine(format!(
                            "search key column not found on {table_name}: {col_name}"
                        ))
                    })?;
                if col.references.is_some() {
                    return Err(DoogatError::SqlEngine(format!(
                        "search key {col_name} is a REFERENCES column; \
                         pick a human-readable column instead"
                    )));
                }
            }
            schema.search_key = if col_lower == "title" {
                None
            } else {
                Some(col_lower.clone())
            };
        } else {
            schema.search_key = None;
        }
        self.update_typedef(table_name, &schema)?;

        // Mirror the new value in `_ddb_meta` so the filter resolver picks
        // up the change without waiting for a full materialization. The
        // `_ddb_meta` row is also re-emitted on every full rebuild via
        // `refresh_boost_table`, so this stays consistent if the user
        // hand-edits the typedef YAML and reindexes later.
        let meta_key = format!("search_key:{table_name}");
        if let Some(ref sk) = schema.search_key {
            self.index.sql_conn().execute(
                "INSERT OR REPLACE INTO _ddb_meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![meta_key, sk],
            )?;
        } else {
            self.index.sql_conn().execute(
                "DELETE FROM _ddb_meta WHERE key = ?1",
                rusqlite::params![meta_key],
            )?;
        }

        let action = if column.is_some() { "set" } else { "dropped" };
        Ok(SqlResult::Ok(format!(
            "search key {action} for {table_name}"
        )))
    }

    /// PRD 00139 §8: `ALTER TABLE x SET SINGLETON`. Idempotent when the
    /// flag is already set. Rejects when the materialized table already
    /// holds more than one row (the operator has to delete duplicates
    /// first; v1 ships no `--keep-most-recent` helper). On success, updates
    /// the typedef YAML and rematerializes the type so the singleton-lock
    /// UNIQUE index lands. Schema reload is triggered automatically by
    /// the rematerialize chain through `_ddb_meta` schemaVersion bump
    /// (PRD 00126).
    pub(in crate::sql_engine) fn handle_set_singleton(&mut self, table_name: &str) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        if schema.singleton {
            return Ok(SqlResult::Ok(format!(
                "singleton already set on {table_name}"
            )));
        }
        // Refuse if the materialized table holds >1 row. Naming the row
        // count in the error gives the operator something to act on.
        let count: i64 = self.index.sql_conn().query_row(
            &format!("SELECT COUNT(*) FROM \"{table_name}\""),
            [],
            |row| row.get(0),
        )?;
        if count > 1 {
            return Err(DoogatError::SqlEngine(format!(
                "ALTER TABLE {table_name} SET SINGLETON: typedef holds {count} rows; delete duplicates and retry"
            )));
        }
        schema.singleton = true;
        self.update_typedef(table_name, &schema)?;
        self.rematerialize_or_defer(table_name)?;
        Ok(SqlResult::Ok(format!("singleton set on {table_name}")))
    }

    /// PRD 00139 §8: `ALTER TABLE x DROP SINGLETON`. Idempotent when the
    /// flag is already cleared. Updates the typedef YAML and
    /// rematerializes so the singleton-lock UNIQUE index disappears.
    pub(in crate::sql_engine) fn handle_drop_singleton(&mut self, table_name: &str) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        if !schema.singleton {
            return Ok(SqlResult::Ok(format!(
                "singleton already cleared on {table_name}"
            )));
        }
        schema.singleton = false;
        self.update_typedef(table_name, &schema)?;
        self.rematerialize_or_defer(table_name)?;
        Ok(SqlResult::Ok(format!("singleton dropped on {table_name}")))
    }

    /// Validate a `title_template` against the current typedef and referenced
    /// typedefs. Rejects multi-hop paths, malformed identifiers, and dotted
    /// tokens whose `col` is not a REFERENCES column on this type or whose
    /// `field` does not exist on the target type.
    fn validate_title_template(&mut self, schema: &TableSchema, template: &str) -> Result<()> {
        use crate::sql_engine::helpers::parse_title_template;

        let placeholders = parse_title_template(template)?;
        for p in &placeholders {
            let Some(field) = &p.field else {
                continue;
            };
            let col_def = schema
                .columns
                .iter()
                .find(|c| c.name == p.col)
                .ok_or_else(|| {
                    DoogatError::SqlEngine(format!(
                        "title_template references {raw}: column '{col}' not found on {table}",
                        raw = p.raw,
                        col = p.col,
                        table = schema.table_name
                    ))
                })?;
            let target_type = col_def.references.as_deref().ok_or_else(|| {
                DoogatError::SqlEngine(format!(
                    "title_template references {raw}: column '{col}' is not a REFERENCES column on {table}",
                    raw = p.raw,
                    col = p.col,
                    table = schema.table_name
                ))
            })?;
            // `title` is always available on any typed doogat.
            if field == "title" {
                continue;
            }
            // Best-effort target-type lookup: if target typedef exists, verify
            // the field. If the target isn't materialized yet, defer to
            // runtime fallback (missing field → empty string).
            let Ok(target_schema) = self.load_schema(target_type) else {
                continue;
            };
            if !target_schema.columns.iter().any(|c| c.name == *field) {
                return Err(DoogatError::SqlEngine(format!(
                    "title_template references {raw}: field '{field}' does not exist on {target_type}",
                    raw = p.raw
                )));
            }
        }
        Ok(())
    }
}
