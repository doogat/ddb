use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::traits::DoogatSource;
use crate::types::ParsedDoogat;

use super::Index;

/// Column names reserved for core doogat fields in materialized tables.
///
/// Exposed at the crate root via `ddb_core::indexer::is_core_column` so
/// downstream crates (the GraphQL server) can distinguish core columns from
/// user-defined ones when iterating `TableSchema.columns` post-PRD 00122.
///
/// This is intentionally narrower than `RESERVED_COLUMNS` in
/// `ddb-core/src/sql_engine/dml.rs`: this set drives whether a user-declared
/// column gets a slot in the materialized typed table. `created_at` and
/// `tags` are NOT in this set because pre-PRD 122 users could legally
/// declare them as typed columns; the doogat pipeline itself doesn't write
/// them into the materialized typed table (`created_at` lives in the
/// internal `doogats` index, `tags` lives in `_ddb_tags`), so the
/// declaration was harmless. The GraphQL server uses its own
/// `BASE_DOOGAT_FIELDS` set to decide which typed-column fields would
/// clobber a base doogat field — see `typed_doogat_to_value` in
/// `ddb-server/src/schema/base_types.rs`.
pub fn is_core_column(name: &str) -> bool {
    matches!(name, "id" | "title" | "type" | "date" | "updated_at")
}

/// Normalize a boolean string to "1" or "0" for SQLite storage.
pub(crate) fn normalize_bool_str(val: &str) -> String {
    match val.to_lowercase().as_str() {
        "true" | "1" | "yes" => "1".to_string(),
        "false" | "0" | "no" => "0".to_string(),
        _ => val.to_string(),
    }
}

/// Build SQL column definitions from schema columns, including core columns.
fn build_column_definitions(columns: &[crate::types::ColumnDef]) -> Vec<String> {
    let mut col_defs = vec![
        "id TEXT PRIMARY KEY".to_string(),
        "title TEXT".to_string(),
        "date TEXT".to_string(),
        "updated_at TEXT".to_string(),
    ];
    for col in columns {
        if is_core_column(&col.name) {
            continue;
        }
        let sql_type = match col.data_type.to_uppercase().as_str() {
            "INTEGER" => "INTEGER",
            "REAL" => "REAL",
            "BOOLEAN" => "INTEGER",
            _ => "TEXT",
        };
        let check = if let Some(ref vals) = col.allowed_values {
            let quoted: Vec<String> = vals
                .iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect();
            format!(
                " CHECK(\"{}\" IS NULL OR \"{}\" IN ({}))",
                col.name,
                col.name,
                quoted.join(", ")
            )
        } else {
            String::new()
        };
        col_defs.push(format!("\"{}\" {}{}", col.name, sql_type, check));
    }
    col_defs
}

/// DDL for creating a junction table for a REFERENCES column.
pub fn junction_table_ddl(table_name: &str, col_name: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS \"{t}_{c}\" (\"{t}_id\" TEXT NOT NULL, \"{c}_id\" TEXT NOT NULL, PRIMARY KEY (\"{t}_id\", \"{c}_id\"))",
        t = table_name,
        c = col_name
    )
}

impl Index {
    /// Drop and recreate a materialized SQLite table from a schema.
    fn drop_and_create_materialized_table(&self, schema: &crate::types::TableSchema) -> Result<()> {
        // Drop junction tables first (before main table)
        for col in &schema.columns {
            if col.references.is_some() {
                self.conn.execute(
                    &format!(
                        "DROP TABLE IF EXISTS \"{}_{}\"",
                        schema.table_name, col.name
                    ),
                    [],
                )?;
            }
        }

        self.conn.execute(
            &format!("DROP TABLE IF EXISTS \"{}\"", schema.table_name),
            [],
        )?;

        let col_defs = build_column_definitions(&schema.columns);
        self.conn.execute(
            &format!(
                "CREATE TABLE \"{}\" ({})",
                schema.table_name,
                col_defs.join(", ")
            ),
            [],
        )?;

        self.create_junction_tables(schema)?;
        self.create_unique_indexes(schema)?;
        self.create_singleton_lock_index(schema)?;

        Ok(())
    }

    fn create_junction_tables(&self, schema: &crate::types::TableSchema) -> Result<()> {
        for col in &schema.columns {
            if col.references.is_some() {
                self.conn
                    .execute(&junction_table_ddl(&schema.table_name, &col.name), [])?;
            }
        }
        Ok(())
    }

    fn create_unique_indexes(&self, schema: &crate::types::TableSchema) -> Result<()> {
        let constraints = match schema.unique_together {
            Some(ref c) => c,
            None => return Ok(()),
        };
        for cols in constraints {
            if cols.is_empty() {
                continue;
            }
            let col_list = cols
                .iter()
                .map(|c| format!("\"{}\"", c))
                .collect::<Vec<_>>()
                .join(", ");
            let index_name = format!("{}_unique_{}", schema.table_name, cols.join("_"));
            self.conn.execute(
                &format!(
                    "CREATE UNIQUE INDEX IF NOT EXISTS \"{}\" ON \"{}\" ({})",
                    index_name, schema.table_name, col_list
                ),
                [],
            )?;
        }
        Ok(())
    }

    /// PRD 00139 §3 layer 3: hard backstop for SINGLETON typedefs. Creates
    /// `CREATE UNIQUE INDEX <table>_singleton_lock ON <table> ((1))` — an
    /// expression index over the literal `1`. SQLite computes `1` for every
    /// row and the UNIQUE constraint then forbids any second row, regardless
    /// of how the row was inserted (typed-create, raw SQL, or a direct write
    /// that bypasses the service path). When `schema.singleton == false`,
    /// this is a no-op; the recreate path drops the index because the table
    /// itself was just dropped at the top of `drop_and_create_materialized_table`.
    fn create_singleton_lock_index(&self, schema: &crate::types::TableSchema) -> Result<()> {
        if !schema.singleton {
            return Ok(());
        }
        let index_name = format!("{}_singleton_lock", schema.table_name);
        self.conn.execute(
            &format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS \"{}\" ON \"{}\" ((1))",
                index_name, schema.table_name
            ),
            [],
        )?;
        Ok(())
    }

    /// Populate a materialized table with data doogats of the given type.
    fn populate_materialized_table(
        &self,
        schema: &crate::types::TableSchema,
        type_name: &str,
        repo: &(impl DoogatSource + ?Sized),
    ) -> Result<()> {
        let mut data_stmt = self.conn.prepare(
            "SELECT id, path FROM doogats WHERE type = ?1 AND path NOT LIKE 'ddb/_conflicts/%'",
        )?;
        let data_doogats: Vec<(String, String)> = data_stmt
            .query_map(params![type_name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        for (doogat_id, doogat_path) in &data_doogats {
            let doogat_content = repo.read_file(doogat_path)?;
            let doogat_parsed = crate::parser::parse(&doogat_content, doogat_path)?;
            self.materialize_row(schema, doogat_id, &doogat_parsed)?;
        }
        Ok(())
    }

    /// Rematerialize a single type's SQLite table.
    /// Loads typedef (if any), infers schema from data, merges, drops/creates table, populates rows.
    pub fn rematerialize_type(
        &self,
        type_name: &str,
        repo: &(impl DoogatSource + ?Sized),
    ) -> Result<()> {
        use crate::sql_engine::schema_from_parsed;

        // Load typedef if exists
        let typedef: Option<crate::types::TableSchema> = {
            let mut stmt = self
                .conn
                .prepare("SELECT path FROM doogats WHERE type = '_typedef' AND title = ?1")?;
            let path: Option<String> = stmt.query_row(params![type_name], |row| row.get(0)).ok();
            path.and_then(|p| {
                let content = repo.read_file(&p).ok()?;
                let parsed = crate::parser::parse(&content, &p).ok()?;
                schema_from_parsed(&parsed).ok()
            })
        };

        // Infer schema from data
        let inferred = self.infer_schema(type_name, repo)?;
        let schema = Self::merge_schemas(typedef, inferred);

        if schema.columns.is_empty() {
            return Ok(());
        }

        self.drop_and_create_materialized_table(&schema)?;
        self.populate_materialized_table(&schema, type_name, repo)?;
        Ok(())
    }

    /// Materialize SQLite tables for all typed doogats using merged schemas.
    /// Returns (tables_materialized, types_inferred).
    pub fn materialize_all_types(&self, repo: &impl DoogatSource) -> Result<(usize, Vec<String>)> {
        let mut tables_materialized = 0;
        let mut types_inferred = Vec::new();

        // Load explicit _typedef schemas
        let typedef_schemas = self.load_all_typedefs(repo);

        // Find all distinct types (excluding _typedef and empty)
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT type FROM doogats WHERE type != '_typedef' AND type != '' AND type IS NOT NULL",
        )?;
        let type_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        for type_name in &type_names {
            let typedef = typedef_schemas.get(type_name.as_str()).cloned();

            let inferred = self.infer_schema(type_name, repo)?;
            let schema = Self::merge_schemas(typedef.clone(), inferred);

            if schema.columns.is_empty() {
                continue;
            }

            if typedef.is_none() {
                // Type inference is tracked in types_inferred and returned to caller
                types_inferred.push(type_name.clone());
            }

            self.drop_and_create_materialized_table(&schema)?;
            self.populate_materialized_table(&schema, type_name, repo)?;
            tables_materialized += 1;
        }

        // Also materialize typedef-only types with no data doogats
        for (type_name, schema) in &typedef_schemas {
            if !type_names.contains(type_name) && !schema.columns.is_empty() {
                self.drop_and_create_materialized_table(schema)?;
                tables_materialized += 1;
            }
        }

        self.drop_orphan_materialized_tables(&typedef_schemas)?;
        self.refresh_boost_table(&typedef_schemas)?;

        Ok((tables_materialized, types_inferred))
    }

    /// Drop any user-table left in SQLite that no longer corresponds to a
    /// typedef (or its reference subtable). Recovers from the partial-rename
    /// case where the git commit landed but the SQLite ALTER did not.
    fn drop_orphan_materialized_tables(
        &self,
        typedef_schemas: &std::collections::HashMap<String, crate::types::TableSchema>,
    ) -> Result<()> {
        let mut keep: std::collections::HashSet<String> =
            std::collections::HashSet::from(["doogats".to_string()]);
        for (name, schema) in typedef_schemas {
            keep.insert(name.clone());
            for col in &schema.columns {
                if col.references.is_some() {
                    keep.insert(format!("{name}_{col_name}", col_name = col.name));
                }
            }
        }

        let mut stmt = self.conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE '\\_ddb\\_%' ESCAPE '\\' \
             AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
        )?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        for name in names {
            if !keep.contains(&name) {
                self.conn
                    .execute(&format!("DROP TABLE IF EXISTS \"{name}\""), [])?;
            }
        }
        Ok(())
    }

    /// Infer a TableSchema from pre-parsed doogats (no git reads).
    pub fn infer_schema_from(
        type_name: &str,
        doogats: &[ParsedDoogat],
    ) -> crate::types::TableSchema {
        use std::collections::HashMap;

        let mut columns: HashMap<String, (crate::types::Zone, Vec<String>)> = HashMap::new();

        for parsed in doogats
            .iter()
            .filter(|z| !z.path.starts_with("ddb/_conflicts/"))
            .filter(|z| z.meta.doogat_type.as_deref() == Some(type_name))
        {
            collect_zone_columns(parsed, &mut columns);
        }

        finalize_schema_columns(type_name, columns)
    }

    /// Populate a materialized table from pre-parsed doogats (no git reads).
    fn populate_materialized_table_from(
        &self,
        schema: &crate::types::TableSchema,
        doogats: &[ParsedDoogat],
    ) -> Result<()> {
        let type_name = &schema.table_name;
        for doogat in doogats
            .iter()
            .filter(|z| !z.path.starts_with("ddb/_conflicts/"))
            .filter(|z| z.meta.doogat_type.as_deref() == Some(type_name.as_str()))
        {
            let id = doogat.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
            self.materialize_row(schema, id, doogat)?;
        }
        Ok(())
    }

    /// Materialize all typed tables from pre-parsed doogats (no git reads).
    pub fn materialize_all_types_from(
        &self,
        doogats: &[ParsedDoogat],
    ) -> Result<(usize, Vec<String>)> {
        let mut tables_materialized = 0;
        let mut types_inferred = Vec::new();

        let typedef_schemas = Self::load_all_typedefs_from(doogats);

        // Find distinct types from the pre-parsed data
        let type_names: Vec<String> = doogats
            .iter()
            .filter(|z| !z.path.starts_with("ddb/_conflicts/"))
            .filter_map(|z| z.meta.doogat_type.as_deref())
            .filter(|t| !t.is_empty() && *t != "_typedef")
            .map(|t| t.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        for type_name in &type_names {
            let typedef = typedef_schemas.get(type_name.as_str()).cloned();

            let inferred = Self::infer_schema_from(type_name, doogats);
            let schema = Self::merge_schemas(typedef.clone(), inferred);

            if schema.columns.is_empty() {
                continue;
            }

            if typedef.is_none() {
                // Type inference is tracked in types_inferred and returned to caller
                types_inferred.push(type_name.clone());
            }

            self.drop_and_create_materialized_table(&schema)?;
            self.populate_materialized_table_from(&schema, doogats)?;
            tables_materialized += 1;
        }

        // Also materialize typedef-only types with no data doogats
        for (type_name, schema) in &typedef_schemas {
            if !type_names.contains(type_name) && !schema.columns.is_empty() {
                self.drop_and_create_materialized_table(schema)?;
                tables_materialized += 1;
            }
        }

        self.refresh_boost_table(&typedef_schemas)?;

        Ok((tables_materialized, types_inferred))
    }

    /// Infer a TableSchema for a type by scanning all data doogats of that type.
    pub fn infer_schema(
        &self,
        type_name: &str,
        repo: &(impl DoogatSource + ?Sized),
    ) -> Result<crate::types::TableSchema> {
        use std::collections::HashMap;

        let mut stmt = self.conn.prepare(
            "SELECT path FROM doogats WHERE type = ?1 AND path NOT LIKE 'ddb/_conflicts/%'",
        )?;
        let paths: Vec<String> = stmt
            .query_map(params![type_name], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut columns: HashMap<String, (crate::types::Zone, Vec<String>)> = HashMap::new();

        for path in &paths {
            let content = repo.read_file(path)?;
            let parsed = crate::parser::parse(&content, path)?;
            collect_zone_columns(&parsed, &mut columns);
        }

        Ok(finalize_schema_columns(type_name, columns))
    }

    /// Merge an explicit typedef schema with an inferred schema.
    /// Typedef columns take precedence; inferred columns fill gaps.
    pub fn merge_schemas(
        typedef: Option<crate::types::TableSchema>,
        inferred: crate::types::TableSchema,
    ) -> crate::types::TableSchema {
        match typedef {
            None => inferred,
            Some(mut td) => {
                let existing_names: std::collections::HashSet<String> =
                    td.columns.iter().map(|c| c.name.clone()).collect();
                for col in inferred.columns {
                    if !existing_names.contains(&col.name) {
                        td.columns.push(col);
                    }
                }
                td
            }
        }
    }

    /// Collect structural consistency warnings during rebuild.
    /// Warnings don't prevent indexing — they're advisory only.
    pub fn collect_consistency_warnings(
        &self,
        repo: &impl DoogatSource,
    ) -> Vec<crate::types::ConsistencyWarning> {
        use crate::types::ConsistencyWarning;

        let mut warnings = Vec::new();

        let paths = match repo.list_doogats() {
            Ok(p) => p,
            Err(_) => return warnings,
        };

        let typedef_schemas = self.load_all_typedefs(repo);

        for path in &paths {
            let content = match repo.read_file(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            match crate::parser::parse(&content, path) {
                Ok(parsed) => {
                    check_cross_zone_duplicates(&parsed, path, &mut warnings);
                    check_required_fields(&parsed, path, &typedef_schemas, &mut warnings);
                }
                Err(e) => {
                    warnings.push(ConsistencyWarning::MalformedYaml {
                        path: path.clone(),
                        error: e.to_string(),
                    });
                }
            }
        }

        warnings
    }

    /// Load all _typedef schemas from the index.
    pub(crate) fn load_all_typedefs(
        &self,
        repo: &dyn DoogatSource,
    ) -> std::collections::HashMap<String, crate::types::TableSchema> {
        use crate::sql_engine::schema_from_parsed;

        let mut schemas = std::collections::HashMap::new();

        let mut stmt = match self
            .conn
            .prepare("SELECT path FROM doogats WHERE type = '_typedef'")
        {
            Ok(s) => s,
            Err(_) => return schemas,
        };

        let paths: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for path in &paths {
            if let Ok(content) = repo.read_file(path) {
                if let Ok(parsed) = crate::parser::parse(&content, path) {
                    if let Ok(schema) = schema_from_parsed(&parsed) {
                        schemas.insert(schema.table_name.clone(), schema);
                    }
                }
            }
        }

        schemas
    }

    /// Enforce RESTRICT semantics for `NOT NULL REFERENCES` columns before a
    /// parent doogat is deleted. Scans every typedef for columns with both
    /// `required = true` and `references = Some(_)`, and asks the
    /// materialized table whether any row currently holds `deleted_id` in
    /// that column. Returns `Err(Validation(..))` at the first blocker found.
    ///
    /// This is the whole RESTRICT check: we do not enumerate the child type
    /// of the deleted doogat, because `references` records only the target
    /// table name, not the column pair, and the materialized table is the
    /// authoritative place where the FK value actually lives. Rows that
    /// reference the deleted id from a nullable FK column are not blocked
    /// here — the existing wikilink-stripping cascade handles those.
    pub(crate) fn check_restrict_blocks_delete(
        &self,
        repo: &dyn DoogatSource,
        deleted_id: &str,
    ) -> Result<()> {
        use crate::error::DoogatError;
        use crate::types::OnDeleteAction;
        use rusqlite::OptionalExtension;

        let schemas = self.load_all_typedefs(repo);
        for (table_name, schema) in &schemas {
            for col in &schema.columns {
                if col.references.is_none() {
                    continue;
                }
                // PRD 00129 §2: only RESTRICT-marked columns block the
                // delete. CASCADE columns are collected separately by
                // [`collect_cascade_children`] for the recursive cascade
                // walk.
                //
                // The historical condition was `required && references` —
                // restricting only on NOT NULL FKs because nullable FKs
                // are handled by the wikilink-strip cascade. Preserve
                // that behavior by skipping nullable RESTRICT columns
                // (they fall through to wikilink stripping).
                if col.on_delete != OnDeleteAction::Restrict || !col.required {
                    continue;
                }
                let sql = format!(
                    "SELECT id FROM \"{}\" WHERE \"{}\" = ?1 LIMIT 1",
                    table_name, col.name
                );
                let blocker: Option<String> = self
                    .conn
                    .query_row(&sql, params![deleted_id], |row| row.get(0))
                    .optional()?;
                if let Some(blocker_id) = blocker {
                    // PRD 00129 §6: structured REFERENCES_VIOLATION code
                    // with the same English wording as before.
                    return Err(DoogatError::references_violation(
                        deleted_id,
                        col.name.clone(),
                        table_name.clone(),
                        blocker_id,
                    ));
                }
            }
        }
        Ok(())
    }

    /// PRD 00129 §2: collect (table, child_id) pairs for every typed-table
    /// row that references `deleted_id` through a column declared with
    /// `ON DELETE CASCADE`. The caller (service::delete_doogat) walks the
    /// returned children recursively to build the full cascade plan.
    pub(crate) fn collect_cascade_children(
        &self,
        repo: &dyn DoogatSource,
        deleted_id: &str,
    ) -> Result<Vec<(String, String)>> {
        use crate::types::OnDeleteAction;
        let mut out = Vec::new();
        let schemas = self.load_all_typedefs(repo);
        for (table_name, schema) in &schemas {
            for col in &schema.columns {
                if col.references.is_none() {
                    continue;
                }
                if col.on_delete != OnDeleteAction::Cascade {
                    continue;
                }
                let sql = format!(
                    "SELECT id FROM \"{}\" WHERE \"{}\" = ?1",
                    table_name, col.name
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let rows = stmt
                    .query_map(params![deleted_id], |row| row.get::<_, String>(0))?
                    .filter_map(|r| r.ok());
                for child_id in rows {
                    out.push((table_name.clone(), child_id));
                }
            }
        }
        Ok(out)
    }

    /// Load typedef schemas from pre-parsed doogats (no git reads).
    fn load_all_typedefs_from(
        doogats: &[ParsedDoogat],
    ) -> std::collections::HashMap<String, crate::types::TableSchema> {
        use crate::sql_engine::schema_from_parsed;

        let mut schemas = std::collections::HashMap::new();
        for z in doogats
            .iter()
            .filter(|z| z.meta.doogat_type.as_deref() == Some("_typedef"))
        {
            if let Ok(schema) = schema_from_parsed(z) {
                schemas.insert(schema.table_name.clone(), schema);
            }
        }
        schemas
    }

    /// Rebuild the `_ddb_boost` table from typedef schemas. Stores
    /// max(search_boost) per type for FTS5 bm25() weighting and the
    /// per-typedef `search_key` column (in `_ddb_meta`) used by the search
    /// filter resolver to substitute the default `title` match column.
    fn refresh_boost_table(
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
    fn materialize_row(
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

/// Check if a key appears in multiple zones across a doogat, emitting warnings for duplicates.
fn check_cross_zone_duplicates(
    parsed: &crate::types::ParsedDoogat,
    path: &str,
    warnings: &mut Vec<crate::types::ConsistencyWarning>,
) {
    use crate::types::ConsistencyWarning;

    let mut seen_keys: std::collections::HashMap<String, &str> = std::collections::HashMap::new();

    for key in parsed.meta.extra.keys() {
        seen_keys.insert(key.clone(), "frontmatter");
    }

    for field in &parsed.inline_fields {
        if field.zone == crate::types::Zone::Reference {
            if let Some(&other_zone) = seen_keys.get(&field.key) {
                if other_zone != "reference" {
                    warnings.push(ConsistencyWarning::CrossZoneDuplicate {
                        path: path.to_string(),
                        key: field.key.clone(),
                    });
                }
            }
        }
    }
}

/// Check if required fields from a typedef are present, emitting warnings for missing ones.
fn check_required_fields(
    parsed: &crate::types::ParsedDoogat,
    path: &str,
    typedef_schemas: &std::collections::HashMap<String, crate::types::TableSchema>,
    warnings: &mut Vec<crate::types::ConsistencyWarning>,
) {
    use crate::types::ConsistencyWarning;

    let type_name = match &parsed.meta.doogat_type {
        Some(t) => t,
        None => return,
    };
    let schema = match typedef_schemas.get(type_name.as_str()) {
        Some(s) => s,
        None => return,
    };

    for col in &schema.columns {
        if !col.required {
            continue;
        }
        let has_value = match col
            .zone
            .as_ref()
            .unwrap_or(&crate::types::Zone::Frontmatter)
        {
            crate::types::Zone::Frontmatter => parsed.meta.extra.contains_key(&col.name),
            crate::types::Zone::Reference => parsed.inline_fields.iter().any(|f| f.key == col.name),
            crate::types::Zone::Body => parsed.body.contains(&format!("## {}", col.name)),
        };
        if !has_value {
            warnings.push(ConsistencyWarning::MissingRequired {
                path: path.to_string(),
                type_name: type_name.clone(),
                field: col.name.clone(),
            });
        }
    }
}

/// Build column names, placeholders, and values for an INSERT from a parsed doogat.
fn extract_column_values(
    schema: &crate::types::TableSchema,
    id: &str,
    doogat: &crate::types::ParsedDoogat,
    updated_at: Option<String>,
) -> (Vec<String>, Vec<String>, Vec<Option<String>>) {
    let mut col_names = vec![
        "id".to_string(),
        "title".to_string(),
        "date".to_string(),
        "updated_at".to_string(),
    ];
    let mut placeholders = vec![
        "?1".to_string(),
        "?2".to_string(),
        "?3".to_string(),
        "?4".to_string(),
    ];
    let mut vals: Vec<Option<String>> = vec![
        Some(id.to_string()),
        doogat.meta.title.clone(),
        doogat.meta.date.clone(),
        updated_at,
    ];

    let mut param_idx = 5;
    for col in &schema.columns {
        if is_core_column(&col.name) {
            continue;
        }
        col_names.push(format!("\"{}\"", col.name));
        placeholders.push(format!("?{}", param_idx));
        param_idx += 1;
        let val = extract_column_value(doogat, col);
        vals.push(if val.is_empty() { None } else { Some(val) });
    }

    (col_names, placeholders, vals)
}

/// Collect frontmatter, body, and reference columns from a parsed doogat into the accumulator.
fn collect_zone_columns(
    parsed: &crate::types::ParsedDoogat,
    columns: &mut std::collections::HashMap<String, (crate::types::Zone, Vec<String>)>,
) {
    use crate::types::Zone;

    for (key, value) in &parsed.meta.extra {
        let inferred_type = infer_yaml_type(value);
        columns
            .entry(key.to_lowercase())
            .or_insert_with(|| (Zone::Frontmatter, Vec::new()))
            .1
            .push(inferred_type);
    }

    for section in &parsed.sections {
        if section.level > 0 {
            columns
                .entry(section.heading.to_lowercase())
                .or_insert_with(|| (Zone::Body, vec!["TEXT".to_string()]));
        }
    }

    for field in &parsed.inline_fields {
        if field.zone == Zone::Reference {
            let entry = columns
                .entry(field.key.to_lowercase())
                .or_insert_with(|| (Zone::Reference, Vec::new()));
            entry.1.push("TEXT".to_string());
        }
    }
}

/// Build final sorted ColumnDef list and TableSchema from collected column data.
fn finalize_schema_columns(
    type_name: &str,
    columns: std::collections::HashMap<String, (crate::types::Zone, Vec<String>)>,
) -> crate::types::TableSchema {
    use crate::types::{ColumnDef, TableSchema};

    let mut cols: Vec<ColumnDef> = columns
        .into_iter()
        .map(|(name, (zone, types))| {
            let data_type = widen_types(&types);
            ColumnDef {
                name,
                data_type,
                references: None,
                zone: Some(zone),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            }
        })
        .collect();

    cols.sort_by(|a, b| a.name.cmp(&b.name));

    TableSchema {
        table_name: type_name.to_string(),
        columns: cols,
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    }
}

/// Extract a column value from a parsed doogat according to zone mapping.
fn extract_column_value(
    doogat: &crate::types::ParsedDoogat,
    col: &crate::types::ColumnDef,
) -> String {
    use crate::types::Zone;

    let zone = col.zone.clone().unwrap_or_else(|| {
        if col.references.is_some() {
            Zone::Reference
        } else if matches!(
            col.data_type.to_uppercase().as_str(),
            "INTEGER" | "REAL" | "BOOLEAN"
        ) {
            Zone::Frontmatter
        } else {
            Zone::Body
        }
    });

    match zone {
        Zone::Reference => {
            let values = extract_multi_reference_values(doogat, &col.name);
            values.join(",")
        }
        Zone::Frontmatter => {
            // Use path navigation for dot/bracket names, flat lookup otherwise
            let val = if col.name.contains('.') || col.name.contains('[') {
                crate::types::get_path_in_map(&doogat.meta.extra, &col.name).ok()
            } else {
                doogat.meta.extra.get(&col.name)
            };
            let is_bool_col = col.data_type.eq_ignore_ascii_case("BOOLEAN");
            val.map(|v| match v {
                crate::types::Value::Number(n) => n.to_string(),
                crate::types::Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
                crate::types::Value::String(s) => {
                    if is_bool_col {
                        normalize_bool_str(s)
                    } else {
                        s.clone()
                    }
                }
                _ => format!("{v:?}"),
            })
            .unwrap_or_default()
        }
        Zone::Body => doogat
            .sections
            .iter()
            .find(|s| s.level > 0 && s.heading.eq_ignore_ascii_case(&col.name))
            .map(|s| s.content.trim().to_string())
            .unwrap_or_default(),
    }
}

/// Extract all reference values for a given column name from a parsed doogat.
///
/// Empty / whitespace-only values are filtered out. PRD 00134 cycle-1 review
/// C1 task #3: `UPDATE … SET col = NULL` collapses to `- col:: [[]]` in the
/// reference zone (`expr_to_string` turns NULL into ""), and without this
/// filter the helper would `INSERT (parent_id, '')` into the junction
/// table, leaving an empty-string ghost row instead of clearing the FK.
/// Filtering here gives a single uniform fix for every caller
/// (`populate_junction_tables`, `sync_junction_tables_for_columns`,
/// `extract_column_values`).
fn extract_multi_reference_values(
    doogat: &crate::types::ParsedDoogat,
    col_name: &str,
) -> Vec<String> {
    doogat
        .inline_fields
        .iter()
        .filter(|f| f.key == col_name && f.zone == crate::types::Zone::Reference)
        .map(|f| f.value.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Infer a SQL data type from a domain Value.
fn infer_yaml_type(value: &crate::types::Value) -> String {
    match value {
        crate::types::Value::Bool(_) => "BOOLEAN".to_string(),
        crate::types::Value::Number(n) => {
            if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 {
                "INTEGER".to_string()
            } else {
                "REAL".to_string()
            }
        }
        crate::types::Value::String(s) => {
            if s.parse::<i64>().is_ok() {
                "INTEGER".to_string()
            } else if s.parse::<f64>().is_ok() {
                "REAL".to_string()
            } else if s == "true" || s == "false" {
                "BOOLEAN".to_string()
            } else {
                "TEXT".to_string()
            }
        }
        _ => "TEXT".to_string(),
    }
}

/// Widen types: if all values agree, use that type; otherwise widen to TEXT.
fn widen_types(types: &[String]) -> String {
    if types.is_empty() {
        return "TEXT".to_string();
    }
    let first = &types[0];
    if types.iter().all(|t| t == first) {
        return first.clone();
    }
    // INTEGER + REAL → REAL
    if types.iter().all(|t| t == "INTEGER" || t == "REAL") {
        return "REAL".to_string();
    }
    "TEXT".to_string()
}
