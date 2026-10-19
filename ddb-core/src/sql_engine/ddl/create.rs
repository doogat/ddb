use rusqlite::params;
use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::{is_core_column, junction_table_ddl};
use crate::parser;
use crate::types::{ColumnDef, TableSchema, Zone};

use crate::sql_engine::builders::build_typedef_doogat;
use crate::sql_engine::helpers::{
    data_type_to_string, extract_allowed_values, extract_default, extract_on_delete,
    extract_references, is_not_null, is_numeric_type, is_reserved_table, is_short_string_type,
    unquote_identifier,
};
use crate::sql_engine::{PendingWrite, SqlEngine, SqlResult};

use super::{
    create_table_with_default_seed_commit_msg, extract_unique_constraints,
    validate_next_partition_refs,
};

impl<'a> SqlEngine<'a> {
    pub(in crate::sql_engine) fn handle_create_table(
        &mut self,
        ct: &sqlparser::ast::CreateTable,
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&ct.name.to_string());

        if is_reserved_table(&table_name) {
            return Err(DoogatError::SqlEngine(format!(
                "reserved table name: {table_name}"
            )));
        }

        // Check if typedef already exists
        let existing: Option<String> = self
            .index
            .sql_conn()
            .query_row(
                "SELECT id FROM doogats WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| row.get(0),
            )
            .ok();
        if existing.is_some() {
            // PRD 00160: this CREATE TABLE is skipped/rejected before
            // extract_columns (the sole consumer of pending_column_zones), so
            // discard only THIS table's parked inline ZONE map — otherwise a
            // skipped `IF NOT EXISTS ... ZONE ...` could leak its zones onto a
            // later CREATE TABLE in the same batch, while a sibling table's
            // zones must survive.
            self.pending_column_zones.remove(&table_name);
            if ct.if_not_exists {
                return Ok(SqlResult::Ok(format!(
                    "table already exists, skipped: {table_name}"
                )));
            }
            return Err(DoogatError::SqlEngine(format!(
                "table already exists: {table_name}"
            )));
        }

        // Extract columns
        let columns = self.extract_columns(&table_name, &ct.columns)?;
        let unique_together = extract_unique_constraints(&ct.constraints, &ct.columns);
        // PRD 00139 §2: take() the pre-parse SINGLETON marker. `Some(_)`
        // flips the flag on the typedef; the inner bool drives T7's
        // auto-seed.
        let pending_singleton = self.pending_singleton.take();
        let schema = TableSchema {
            table_name: table_name.clone(),
            columns,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: Some("ddl".into()),
            unique_together,
            search_key: None,
            singleton: pending_singleton.is_some(),
        };
        let auto_seed = pending_singleton == Some(true);

        // PRD 00139 §7: validate the auto-seed precondition before any
        // file write. Every non-core, NOT NULL column must have a
        // column-level `default_value` so the seed row can be built
        // without user input. Failing here keeps the typedef out of git
        // entirely on the operator's bad CREATE TABLE.
        if auto_seed {
            let missing: Vec<String> = schema
                .columns
                .iter()
                .filter(|c| !is_core_column(&c.name) && c.required && c.default_value.is_none())
                .map(|c| c.name.clone())
                .collect();
            if !missing.is_empty() {
                return Err(DoogatError::SqlEngine(format!(
                    "SINGLETON DEFAULT VALUES requires every non-nullable column to have a column-level default; missing: {}",
                    missing.join(", ")
                )));
            }
        }

        // Build the typedef doogat content.
        let typedef_id = self.unique_id()?;
        let schema_doogat = build_typedef_doogat(&typedef_id, &schema);
        let typedef_content = parser::serialize(&schema_doogat);
        let typedef_path = format!("ddb/_typedef/{}.md", typedef_id.0);

        // PRD 00139 §7: when auto-seeding, build the seed row content
        // here so typedef + seed land in a single git commit (single
        // CRDT-sync boundary). Other nodes inherit the seeded row via
        // normal sync — they do NOT auto-seed locally on typedef receipt.
        let seed_artifacts = if auto_seed {
            let seed_id = self.unique_id()?;
            let mut seed_values: BTreeMap<String, String> = BTreeMap::new();
            for col in &schema.columns {
                if is_core_column(&col.name) {
                    continue;
                }
                if let Some(ref dv) = col.default_value {
                    seed_values.insert(col.name.clone(), dv.clone());
                }
            }
            let ref_folder_types = self.ref_folder_types(&schema);
            let seed_doogat = crate::sql_engine::builders::build_data_doogat(
                &seed_id,
                &schema,
                &seed_values,
                &ref_folder_types,
                Some(self.index.sql_conn()),
            );
            let seed_content = parser::serialize(&seed_doogat);
            let seed_path = if schema.folder {
                format!("ddb/{}/{}.md", schema.table_name, seed_id.0)
            } else {
                format!("ddb/{}.md", seed_id.0)
            };
            Some((seed_id, seed_path, seed_content, seed_values))
        } else {
            None
        };

        // Atomic commit: typedef + (optional) seed row in one git commit.
        let commit_msg = if auto_seed {
            create_table_with_default_seed_commit_msg(&table_name)
        } else {
            format!("create table {table_name}")
        };
        // Inside a transaction the typedef (and optional seed) git writes are
        // buffered and flushed as one commit on COMMIT (PRD 00161 task 10); the
        // in-memory `index_doogat` + `create_materialized_table` below still run
        // under the SAVEPOINT, so the type is queryable within the transaction.
        match (&mut self.txn, &seed_artifacts) {
            (Some(buf), Some((_, seed_path, seed_content, _))) => {
                buf.writes.push(PendingWrite {
                    path: typedef_path.clone(),
                    content: typedef_content.clone(),
                });
                buf.writes.push(PendingWrite {
                    path: seed_path.clone(),
                    content: seed_content.clone(),
                });
            }
            (Some(buf), None) => {
                buf.writes.push(PendingWrite {
                    path: typedef_path.clone(),
                    content: typedef_content.clone(),
                });
            }
            (None, Some((_, seed_path, seed_content, _))) => {
                self.repo.commit_files(
                    &[
                        (typedef_path.as_str(), typedef_content.as_str()),
                        (seed_path.as_str(), seed_content.as_str()),
                    ],
                    &commit_msg,
                )?;
            }
            (None, None) => {
                self.repo
                    .commit_file(&typedef_path, &typedef_content, &commit_msg)?;
            }
        }

        // Index the typedef doogat first so subsequent materialization
        // reads it back through the standard chain.
        let parsed = parser::parse(&typedef_content, &typedef_path)?;
        self.index.index_doogat(&parsed)?;

        // Create the materialized SQLite table and any attached indexes,
        // including the singleton-lock when `schema.singleton` is true.
        self.create_materialized_table(&schema)?;

        // Index + materialize the seed row last so the singleton-lock
        // index it lives under is already in place.
        if let Some((seed_id, seed_path, seed_content, seed_values)) = seed_artifacts {
            let seed_parsed = parser::parse(&seed_content, &seed_path)?;
            self.index.index_doogat(&seed_parsed)?;
            self.insert_materialized_row(&schema, &seed_id.0, &seed_values)?;
        }

        Ok(SqlResult::Ok(format!("table {table_name} created")))
    }

    fn extract_columns(
        &mut self,
        table_name: &str,
        cols: &[sqlparser::ast::ColumnDef],
    ) -> Result<Vec<ColumnDef>> {
        let mut out = Vec::new();
        // Take only this table's parked inline zones; later CREATE TABLE
        // statements in the same batch keep theirs (PRD 00160).
        let declared_zones = self
            .pending_column_zones
            .remove(table_name)
            .unwrap_or_default();
        for col in cols {
            let name = col.name.value.to_lowercase();
            if name == "id" || name == "type" {
                continue; // implicit auto-managed columns, skip
            }
            // `title` is special: stored in meta.title and on the materialized
            // table as a top-level column, not as a typed-table column. We
            // still keep it in `schema.columns` so its declared constraints
            // (NOT NULL, VARCHAR(N)) flow into the row-validation pre-check.
            // Downstream code that walks `schema.columns` skips core columns
            // via `is_core_column`.
            let data_type = data_type_to_string(&col.data_type);
            let references = extract_references(&col.options);
            // An inline ZONE applies to any schema column, core columns
            // included, so it round-trips identically to a post-hoc
            // `ALTER TABLE ... SET ZONE` (which also sets the zone on any
            // schema column). Functionally inert on core columns (they are
            // not materialized as typed-table columns) but keeps parity.
            let zone = if let Some(z) = declared_zones.get(&name) {
                Some(z.clone())
            } else if references.is_some() {
                Some(Zone::Reference)
            } else if is_numeric_type(&data_type) || is_short_string_type(&col.data_type) {
                Some(Zone::Frontmatter)
            } else {
                Some(Zone::Body)
            };
            let allowed_values = extract_allowed_values(&col.data_type);
            let default_value = extract_default(&col.options)?;
            if let Some(ref dv) = default_value {
                if (dv == "NEXT" || dv.starts_with("NEXT("))
                    && !data_type.eq_ignore_ascii_case("integer")
                {
                    return Err(DoogatError::SqlEngine(format!(
                        "DEFAULT NEXT is only valid on INTEGER columns, not {data_type}"
                    )));
                }
            }
            // PRD 00129 §2: pull `ON DELETE` action off the FK option. For
            // non-REFERENCES columns it stays at the default RESTRICT.
            // SET NULL / SET DEFAULT / NO ACTION reject here so callers
            // get a clear error rather than silent-RESTRICT-fallback.
            let on_delete = extract_on_delete(&col.options)?;
            out.push(ColumnDef {
                name,
                data_type,
                references,
                zone,
                required: is_not_null(&col.options),
                search_boost: None,
                allowed_values,
                default_value,
                on_delete,
            });
        }

        validate_next_partition_refs(&out)?;

        Ok(out)
    }

    fn create_materialized_table(&mut self, schema: &TableSchema) -> Result<()> {
        let mut col_defs = vec![
            "id TEXT PRIMARY KEY".to_string(),
            "title TEXT".to_string(),
            "date TEXT".to_string(),
            "updated_at TEXT".to_string(),
        ];
        for col in &schema.columns {
            if is_core_column(&col.name) {
                continue;
            }
            let sql_type = match col.data_type.to_uppercase().as_str() {
                "INTEGER" => "INTEGER",
                "REAL" => "REAL",
                "BOOLEAN" => "INTEGER", // SQLite stores booleans as integers
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
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            schema.table_name,
            col_defs.join(", ")
        );
        self.index.sql_conn().execute(&sql, [])?;

        // Create junction tables for REFERENCES columns
        for col in &schema.columns {
            if col.references.is_some() {
                self.index
                    .sql_conn()
                    .execute(&junction_table_ddl(&schema.table_name, &col.name), [])?;
            }
        }

        // Create unique indexes for UNIQUE constraints
        if let Some(ref constraints) = schema.unique_together {
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
                self.index.sql_conn().execute(
                    &format!(
                        "CREATE UNIQUE INDEX IF NOT EXISTS \"{}\" ON \"{}\" ({})",
                        index_name, schema.table_name, col_list
                    ),
                    [],
                )?;
            }
        }

        if schema.singleton {
            self.index.sql_conn().execute(
                &format!(
                    "CREATE UNIQUE INDEX IF NOT EXISTS \"{}_singleton_lock\" ON \"{}\" ((1))",
                    schema.table_name, schema.table_name
                ),
                [],
            )?;
        }

        Ok(())
    }
}
