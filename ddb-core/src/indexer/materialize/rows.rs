use rusqlite::params;

use crate::error::{DoogatError, Result};

use super::super::Index;
use super::schema_helpers::{extract_column_values, extract_multi_reference_values};

impl Index {
    /// Rebuild the `_ddb_boost` table from typedef schemas. Stores
    /// max(search_boost) per type for FTS5 bm25() weighting and the
    /// per-typedef `search_key` column (in `_ddb_meta`) used by the search
    /// filter resolver to substitute the default `title` match column.
    pub(super) fn refresh_boost_table(
        &self,
        schemas: &std::collections::HashMap<String, crate::types::TableSchema>,
    ) -> Result<()> {
        self.conn.execute("DELETE FROM _ddb_boost", [])?;
        // Wipe stale search_key entries; we re-emit fresh rows below.
        self.conn
            .execute("DELETE FROM _ddb_meta WHERE key LIKE 'search_key:%'", [])?;
        for (type_name, schema) in schemas {
            let max_boost = schema
                .columns
                .iter()
                .filter_map(|c| c.search_boost)
                .fold(1.0_f64, f64::max);
            if max_boost > 1.0 {
                self.conn.execute(
                    "INSERT INTO _ddb_boost (type_name, max_boost) VALUES (?1, ?2)",
                    rusqlite::params![type_name, max_boost],
                )?;
            }
            if let Some(ref sk) = schema.search_key {
                self.conn.execute(
                    "INSERT OR REPLACE INTO _ddb_meta (key, value) VALUES (?1, ?2)",
                    rusqlite::params![format!("search_key:{type_name}"), sk],
                )?;
            }
        }
        Ok(())
    }

    /// Re-materialize a single doogat row (main table + junction tables).
    /// Clears old junction rows before inserting fresh ones.
    ///
    /// PRD 00134 blind-review I4: the DELETE-old-junctions + INSERT-row +
    /// INSERT-new-junctions trio runs inside a SAVEPOINT so a failure in
    /// `materialize_row` (NOT NULL/CHECK violation, etc.) rolls back the
    /// junction DELETEs. Without the savepoint, junction rows would be
    /// permanently lost for the failed doogat until the next full rebuild.
    /// SQLite supports nested savepoints, so callers that already hold one
    /// (e.g. `update_indexes_atomically`) keep working.
    pub(crate) fn materialize_single(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        doogat: &crate::types::ParsedDoogat,
    ) -> Result<()> {
        if doogat.path.starts_with("ddb/_conflicts/") {
            return Ok(());
        }
        // PRD 00134 blind-review C1 follow-up: typedefs installed via
        // `install_bundled_type` (or a not-yet-rebuilt git pull) are
        // registered as YAML doogats but the materialized SQLite table
        // doesn't exist until a subsequent `reindex` / `rebuild`. Skip
        // here in that pre-rebuild state — the next reindex will
        // populate the table from scratch. This matches the prior
        // contract of the install-bundled-type path.
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master \
                 WHERE type = 'table' AND name = ?1",
                rusqlite::params![&schema.table_name],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !table_exists {
            return Ok(());
        }
        self.with_savepoint("materialize_single", || {
            // Clear old junction rows for this doogat
            for col in &schema.columns {
                if col.references.is_some() {
                    self.conn.execute(
                        &format!(
                            "DELETE FROM \"{t}_{c}\" WHERE \"{t}_id\" = ?1",
                            t = schema.table_name,
                            c = col.name
                        ),
                        params![id],
                    )?;
                }
            }
            self.materialize_row(schema, id, doogat)
        })
    }

    /// Insert a single data doogat's values into a materialized table.
    pub(super) fn materialize_row(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        doogat: &crate::types::ParsedDoogat,
    ) -> Result<()> {
        if schema.singleton {
            let existing_id: Option<String> = self
                .conn
                .query_row(
                    &format!(
                        "SELECT id FROM \"{}\" WHERE id != ?1 LIMIT 1",
                        schema.table_name
                    ),
                    params![id],
                    |row| row.get(0),
                )
                .ok();
            if let Some(existing_id) = existing_id {
                return Err(DoogatError::singleton_violation(
                    schema.table_name.clone(),
                    existing_id,
                ));
            }
        }

        let updated_at: Option<String> = self
            .conn
            .query_row(
                "SELECT updated_at FROM doogats WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .ok();

        let (col_names, placeholders, vals) = extract_column_values(schema, id, doogat, updated_at);

        let sql = format!(
            "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
            schema.table_name,
            col_names.join(", "),
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = vals
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        self.conn.execute(&sql, params.as_slice())?;

        self.populate_junction_tables(schema, id, doogat)?;

        Ok(())
    }

    /// Insert junction table rows for REFERENCES columns.
    ///
    /// Also used by the SQL INSERT path in `SqlEngine::build_and_index_row`.
    pub(crate) fn populate_junction_tables(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        doogat: &crate::types::ParsedDoogat,
    ) -> Result<()> {
        for col in &schema.columns {
            if col.references.is_some() {
                let ref_values = extract_multi_reference_values(doogat, &col.name);
                for ref_id in &ref_values {
                    self.conn.execute(
                        &format!(
                            "INSERT OR IGNORE INTO \"{t}_{c}\" (\"{t}_id\", \"{c}_id\") VALUES (?1, ?2)",
                            t = schema.table_name,
                            c = col.name
                        ),
                        params![id, ref_id],
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn sync_junction_tables_for_columns(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        doogat: &crate::types::ParsedDoogat,
        changed_cols: &[&str],
    ) -> Result<()> {
        for col in &schema.columns {
            if changed_cols.iter().any(|c| *c == col.name) && col.references.is_some() {
                self.conn.execute(
                    &format!(
                        "DELETE FROM \"{t}_{c}\" WHERE \"{t}_id\" = ?1",
                        t = schema.table_name,
                        c = col.name
                    ),
                    params![id],
                )?;

                let ref_values = extract_multi_reference_values(doogat, &col.name);
                for ref_id in &ref_values {
                    self.conn.execute(
                        &format!(
                            "INSERT OR IGNORE INTO \"{t}_{c}\" (\"{t}_id\", \"{c}_id\") VALUES (?1, ?2)",
                            t = schema.table_name,
                            c = col.name
                        ),
                        params![id, ref_id],
                    )?;
                }
            }
        }
        Ok(())
    }
}
