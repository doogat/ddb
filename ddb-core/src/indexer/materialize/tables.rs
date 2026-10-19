use rusqlite::params;

use crate::error::Result;
use crate::traits::DoogatSource;
use crate::types::ParsedDoogat;

use super::super::Index;
use super::schema_helpers::{collect_zone_columns, finalize_schema_columns};
use super::{build_column_definitions, junction_table_ddl};

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
        // PRD 00139 cycle-3 #5: ORDER BY path so SINGLETON_VIOLATION's
        // existing_id is deterministic across runs and matches what
        // populate_materialized_table_from produces (which uses
        // list_doogats(), alphabetically sorted). The singleton_layers
        // parity test asserts layer-1/2/3 errors are byte-identical, so
        // both materialize paths must process doogats in the same order.
        let mut data_stmt = self.conn.prepare(
            "SELECT id, path FROM doogats WHERE type = ?1 AND path NOT LIKE 'ddb/_conflicts/%' ORDER BY path ASC",
        )?;
        // A row-decode failure here means the SQLite index is corrupt. Warn
        // rather than silently drop the row (PRD 00140). Triggering this needs
        // SQLite-level corruption, so the warn arm is covered by inspection.
        let data_doogats: Vec<(String, String)> = data_stmt
            .query_map(params![type_name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| match r {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, type_name, "populate_materialized_table: doogat row decode failed; row skipped");
                    None
                }
            })
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
            let path: Option<String> = match stmt.query_row(params![type_name], |row| row.get(0)) {
                Ok(p) => Some(p),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => {
                    tracing::warn!(error = %e, type_name, "rematerialize_type: typedef path lookup failed, falling back to inferred schema");
                    None
                }
            };
            match path {
                None => None,
                Some(p) => match repo.read_file(&p) {
                    Err(e) => {
                        tracing::warn!(error = %e, type_name, path = %p, "rematerialize_type: typedef file read failed");
                        None
                    }
                    Ok(content) => match crate::parser::parse(&content, &p) {
                        Err(e) => {
                            tracing::warn!(error = %e, type_name, path = %p, "rematerialize_type: typedef parse failed");
                            None
                        }
                        Ok(parsed) => match schema_from_parsed(&parsed) {
                            Err(e) => {
                                tracing::warn!(error = %e, type_name, "rematerialize_type: schema extraction failed");
                                None
                            }
                            Ok(s) => Some(s),
                        },
                    },
                },
            }
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
        // Atomic to concurrent readers: the per-type drop+recreate in the inner
        // method must commit as one unit, or a second connection (WAL) can read
        // between a committed `DROP TABLE` and the matching `CREATE TABLE` and
        // see "no such table" — the residual §55.A cross-process race where two
        // racing `ddb create` losers surfaced "no such table" instead of the
        // structured SINGLETON message. `with_immediate_transaction` is
        // nesting-tolerant: when a SINGLETON write's BEGIN IMMEDIATE is already
        // open it runs inline and that outer transaction owns the commit.
        super::super::with_immediate_transaction(&self.conn, || self.materialize_all_types_inner(repo))
    }

    fn materialize_all_types_inner(
        &self,
        repo: &impl DoogatSource,
    ) -> Result<(usize, Vec<String>)> {
        let mut tables_materialized = 0;
        let mut types_inferred = Vec::new();

        // Load explicit _typedef schemas
        let typedef_schemas = self.load_all_typedefs(repo);

        // Find all distinct types (excluding _typedef and empty)
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT type FROM doogats WHERE type != '_typedef' AND type != '' AND type IS NOT NULL",
        )?;
        // Warn on row-decode failure instead of dropping the type silently
        // (PRD 00140); the warn arm requires a corrupt index to reach.
        let type_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| match r {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "materialize_all_types: type-name row decode failed; type skipped");
                    None
                }
            })
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
        // Warn on row-decode failure instead of dropping the table name
        // silently (PRD 00140); the warn arm requires a corrupt index to reach.
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| match r {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "drop_orphan_materialized_tables: table-name row decode failed; table skipped");
                    None
                }
            })
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
}
