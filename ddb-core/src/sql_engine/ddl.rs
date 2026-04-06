use rusqlite::params;
use sqlparser::ast::{AlterTableOperation, ObjectType};

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::{is_core_column, junction_table_ddl};
use crate::parser;
use crate::types::{ColumnDef, DoogatId, TableSchema, Zone};

use super::builders::{build_typedef_doogat, rename_key_in_doogat, schema_from_parsed};
use super::helpers::{
    data_type_to_string, extract_allowed_values, extract_default, extract_references,
    is_numeric_type, is_reserved_table, is_short_string_type, unquote_identifier,
};
use super::{SqlEngine, SqlResult};

impl<'a> SqlEngine<'a> {
    pub(super) fn handle_create_table(
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
            .conn
            .query_row(
                "SELECT id FROM doogats WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| row.get(0),
            )
            .ok();
        if existing.is_some() {
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
        let columns = self.extract_columns(&ct.columns)?;
        let schema = TableSchema {
            table_name: table_name.clone(),
            columns,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: Some("ddl".into()),
            unique_together: None,
        };

        // Build and commit typedef doogat
        let id = self.unique_id()?;
        let schema_doogat = build_typedef_doogat(&id, &schema);
        let content = parser::serialize(&schema_doogat);
        let path = format!("ddb/_typedef/{}.md", id.0);
        self.repo
            .commit_file(&path, &content, &format!("create table {table_name}"))?;

        // Index the typedef doogat
        let parsed = parser::parse(&content, &path)?;
        self.index.index_doogat(&parsed)?;

        // Create materialized SQLite table
        self.create_materialized_table(&schema)?;

        Ok(SqlResult::Ok(format!("table {table_name} created")))
    }

    fn extract_columns(&mut self, cols: &[sqlparser::ast::ColumnDef]) -> Result<Vec<ColumnDef>> {
        let mut out = Vec::new();
        for col in cols {
            let name = col.name.value.to_lowercase();
            if name == "id" || name == "type" || name == "title" {
                continue; // implicit columns, skip
            }
            let data_type = data_type_to_string(&col.data_type);
            let references = extract_references(&col.options);
            let zone = if references.is_some() {
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
            out.push(ColumnDef {
                name,
                data_type,
                references,
                zone,
                required: false,
                search_boost: None,
                allowed_values,
                default_value,
            });
        }

        // Validate NEXT(col) partition columns exist in the table
        let col_names: Vec<&str> = out.iter().map(|c| c.name.as_str()).collect();
        for col in &out {
            if let Some(ref dv) = col.default_value {
                if dv.starts_with("NEXT(") && dv.ends_with(')') {
                    let partition_col = &dv[5..dv.len() - 1];
                    if !col_names.contains(&partition_col) {
                        return Err(DoogatError::SqlEngine(format!(
                            "DEFAULT NEXT({partition_col}): column '{partition_col}' not found in table"
                        )));
                    }
                }
            }
        }

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
        self.index.conn.execute(&sql, [])?;

        // Create junction tables for REFERENCES columns
        for col in &schema.columns {
            if col.references.is_some() {
                self.index
                    .conn
                    .execute(&junction_table_ddl(&schema.table_name, &col.name), [])?;
            }
        }

        Ok(())
    }

    pub(super) fn handle_alter_table(
        &mut self,
        name: &sqlparser::ast::ObjectName,
        operations: &[AlterTableOperation],
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&name.to_string());
        let (typedef_id, typedef_path) = self.load_typedef_location(&table_name)?;
        let mut schema = self.load_schema(&table_name)?;

        for op in operations {
            match op {
                AlterTableOperation::AddColumn { column_def, .. } => {
                    let col_name = column_def.name.value.to_lowercase();
                    if schema.columns.iter().any(|c| c.name == col_name) {
                        return Err(DoogatError::SqlEngine(format!(
                            "column already exists: {col_name}"
                        )));
                    }
                    let dt = data_type_to_string(&column_def.data_type);
                    let refs = extract_references(&column_def.options);
                    let default_value = extract_default(&column_def.options)?;
                    // Validate NEXT defaults on ALTER TABLE ADD COLUMN
                    if let Some(ref dv) = default_value {
                        if (dv == "NEXT" || dv.starts_with("NEXT("))
                            && !dt.eq_ignore_ascii_case("integer")
                        {
                            return Err(DoogatError::SqlEngine(format!(
                                "DEFAULT NEXT is only valid on INTEGER columns, not {dt}"
                            )));
                        }
                        if dv.starts_with("NEXT(") && dv.ends_with(')') {
                            let partition_col = &dv[5..dv.len() - 1];
                            let all_cols: Vec<&str> =
                                schema.columns.iter().map(|c| c.name.as_str()).collect();
                            if !all_cols.contains(&partition_col) && partition_col != col_name {
                                return Err(DoogatError::SqlEngine(format!(
                                    "DEFAULT NEXT({partition_col}): column '{partition_col}' not found in table"
                                )));
                            }
                        }
                    }
                    let zone = if refs.is_some() {
                        Some(Zone::Reference)
                    } else if is_numeric_type(&dt) || is_short_string_type(&column_def.data_type) {
                        Some(Zone::Frontmatter)
                    } else {
                        Some(Zone::Body)
                    };
                    schema.columns.push(ColumnDef {
                        name: col_name,
                        data_type: dt,
                        zone,
                        required: false,
                        search_boost: None,
                        references: refs,
                        allowed_values: extract_allowed_values(&column_def.data_type),
                        default_value,
                    });
                }
                AlterTableOperation::DropColumn {
                    column_name,
                    if_exists,
                    ..
                } => {
                    let col_name = column_name.value.to_lowercase();
                    let pos = schema.columns.iter().position(|c| c.name == col_name);
                    match pos {
                        Some(i) => {
                            schema.columns.remove(i);
                        }
                        None if *if_exists => {}
                        None => {
                            return Err(DoogatError::SqlEngine(format!(
                                "column not found: {col_name}"
                            )));
                        }
                    }
                }
                AlterTableOperation::RenameColumn {
                    old_column_name,
                    new_column_name,
                } => {
                    return self.handle_rename_column(
                        &table_name,
                        &typedef_id,
                        &typedef_path,
                        &mut schema,
                        &old_column_name.value.to_lowercase(),
                        &new_column_name.value.to_lowercase(),
                    );
                }
                other => {
                    return Err(DoogatError::SqlEngine(format!(
                        "unsupported ALTER TABLE operation: {other}"
                    )));
                }
            }
        }

        self.update_typedef(&table_name, &schema)?;
        self.index.rematerialize_type(&table_name, self.repo)?;

        Ok(SqlResult::Ok(format!("table {table_name} altered")))
    }

    /// Serialize a modified TableSchema back to its typedef doogat, commit to Git, re-index.
    fn update_typedef(&mut self, table_name: &str, schema: &TableSchema) -> Result<()> {
        let (typedef_id, typedef_path) = self.load_typedef_location(table_name)?;
        let id = DoogatId(typedef_id);
        let schema_doogat = build_typedef_doogat(&id, schema);
        let content = parser::serialize(&schema_doogat);
        self.repo.commit_file(
            &typedef_path,
            &content,
            &format!("alter table {table_name}"),
        )?;
        let parsed = parser::parse(&content, &typedef_path)?;
        self.index.index_doogat(&parsed)?;
        Ok(())
    }

    pub(super) fn handle_set_zone(
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
        self.index.rematerialize_type(table_name, self.repo)?;
        Ok(SqlResult::Ok(format!(
            "zone set to {zone_str} for {col_lower} in {table_name}"
        )))
    }

    pub(super) fn handle_title_template(
        &mut self,
        table_name: &str,
        template: Option<&str>,
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        schema.title_template = template.map(String::from);
        self.update_typedef(table_name, &schema)?;
        let action = if template.is_some() { "set" } else { "dropped" };
        Ok(SqlResult::Ok(format!(
            "title template {action} for {table_name}"
        )))
    }

    fn handle_rename_column(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        schema: &mut TableSchema,
        old_name: &str,
        new_name: &str,
    ) -> Result<SqlResult> {
        if schema.columns.iter().any(|c| c.name == new_name) {
            return Err(DoogatError::SqlEngine(format!(
                "column already exists: {new_name}"
            )));
        }
        let col = schema
            .columns
            .iter_mut()
            .find(|c| c.name == old_name)
            .ok_or_else(|| DoogatError::SqlEngine(format!("column not found: {old_name}")))?;
        let zone = col.effective_zone();
        col.name = new_name.to_string();

        let id = DoogatId(typedef_id.to_string());
        let schema_doogat = build_typedef_doogat(&id, schema);
        let typedef_content = parser::serialize(&schema_doogat);

        let data_doogats = self.resolve_matching_ids(table_name, &None)?;
        let mut files: Vec<(String, String)> = Vec::with_capacity(data_doogats.len() + 1);
        files.push((typedef_path.to_string(), typedef_content.clone()));

        for (_, path) in &data_doogats {
            let content = self.repo.read_file(path)?;
            let mut parsed = parser::parse(&content, path)?;
            rename_key_in_doogat(&mut parsed, old_name, new_name, &zone);
            files.push((path.clone(), parser::serialize(&parsed)));
        }

        let file_refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_files(
            &file_refs,
            &format!("alter table {table_name} rename {old_name} to {new_name}"),
        )?;

        let parsed_typedef = parser::parse(&typedef_content, typedef_path)?;
        self.index.index_doogat(&parsed_typedef)?;
        for (_, path) in &data_doogats {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_doogat(&parsed)?;
        }
        self.index.rematerialize_type(table_name, self.repo)?;

        Ok(SqlResult::Ok(format!(
            "renamed {old_name} to {new_name} in {table_name}"
        )))
    }

    pub(super) fn handle_drop(
        &mut self,
        object_type: &ObjectType,
        if_exists: bool,
        names: &[sqlparser::ast::ObjectName],
        cascade: bool,
    ) -> Result<SqlResult> {
        if *object_type != ObjectType::Table {
            return Err(DoogatError::SqlEngine(format!(
                "DROP {} not supported, only DROP TABLE",
                object_type
            )));
        }

        for name in names {
            let table_name = unquote_identifier(&name.to_string());
            self.handle_drop_table(&table_name, if_exists, cascade)?;
        }

        Ok(SqlResult::Ok(format!("dropped {} table(s)", names.len())))
    }

    fn handle_drop_table(
        &mut self,
        table_name: &str,
        if_exists: bool,
        cascade: bool,
    ) -> Result<()> {
        // Locate typedef
        let typedef_loc = match self.load_typedef_location(table_name) {
            Ok(loc) => loc,
            Err(_) if if_exists => return Ok(()),
            Err(e) => return Err(e),
        };
        let (typedef_id, typedef_path) = typedef_loc;

        // Load schema before git deletes (needed for junction table cleanup)
        let schema = self.load_schema(table_name).ok();

        // Find all data doogats of this type
        let data_doogats: Vec<(String, String)> = {
            let mut stmt = self
                .index
                .conn
                .prepare("SELECT id, path FROM doogats WHERE type = ?1")?;
            let rows: Vec<(String, String)> = stmt
                .query_map(params![table_name], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };

        if cascade {
            // Delete typedef + all data doogats
            let mut paths: Vec<&str> = vec![&typedef_path];
            for (_, path) in &data_doogats {
                paths.push(path);
            }
            self.repo
                .delete_files(&paths, &format!("drop table {table_name} cascade"))?;

            // Remove from index
            self.index.remove_doogat(&typedef_id)?;
            for (id, _) in &data_doogats {
                self.index.remove_doogat(id)?;
            }
        } else {
            // Rewrite data doogats to remove type field, then delete typedef
            let mut writes: Vec<(String, String)> = Vec::new();
            for (_, path) in &data_doogats {
                let content = self.repo.read_file(path)?;
                let mut parsed = parser::parse(&content, path)?;
                parsed.meta.doogat_type = None;
                writes.push((path.clone(), parser::serialize(&parsed)));
            }

            let write_refs: Vec<(&str, &str)> = writes
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            self.repo.commit_batch(
                &write_refs,
                &[&typedef_path],
                &format!("drop table {table_name}"),
            )?;

            // Re-index modified data doogats
            for (_, path) in &data_doogats {
                let content = self.repo.read_file(path)?;
                let parsed = parser::parse(&content, path)?;
                self.index.index_doogat(&parsed)?;
            }
            // Remove typedef from index
            self.index.remove_doogat(&typedef_id)?;
        }

        // Drop junction tables for REFERENCES columns
        if let Some(ref schema) = schema {
            for col in &schema.columns {
                if col.references.is_some() {
                    self.index.conn.execute(
                        &format!(
                            "DROP TABLE IF EXISTS \"{table_name}_{col_name}\"",
                            col_name = col.name
                        ),
                        [],
                    )?;
                }
            }
        }

        // Drop materialized SQLite table
        self.index
            .conn
            .execute(&format!("DROP TABLE IF EXISTS \"{table_name}\""), [])?;

        Ok(())
    }

    pub(super) fn load_typedef_location(
        &mut self,
        table_name: &str,
    ) -> Result<(String, String)> {
        self.index
            .conn
            .query_row(
                "SELECT id, path FROM doogats WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| DoogatError::SqlEngine(format!("table not found: {table_name}")))
    }

    pub(super) fn load_schema(&mut self, table_name: &str) -> Result<TableSchema> {
        let (_id, path) = self.load_typedef_location(table_name)?;
        let content = self.repo.read_file(&path)?;
        let parsed = parser::parse(&content, &path)?;
        schema_from_parsed(&parsed)
    }
}
