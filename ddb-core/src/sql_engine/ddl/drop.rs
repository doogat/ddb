use rusqlite::params;
use sqlparser::ast::ObjectType;

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::TableSchema;

use crate::sql_engine::helpers::unquote_identifier;
use crate::sql_engine::{SqlEngine, SqlResult};

impl<'a> SqlEngine<'a> {
    pub(in crate::sql_engine) fn handle_drop(
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
        let typedef_loc = match self.load_typedef_location(table_name) {
            Ok(loc) => loc,
            Err(_) if if_exists => return Ok(()),
            Err(e) => return Err(e),
        };
        let (typedef_id, typedef_path) = typedef_loc;

        // Load schema before git deletes (needed for junction table cleanup)
        let schema = self.load_schema(table_name).ok();
        let data_doogats = self.find_type_data_doogats(table_name)?;

        if cascade {
            self.cascade_delete_doogats(&typedef_path, &typedef_id, &data_doogats, table_name)?;
        } else {
            self.soft_drop_doogats(&typedef_id, &typedef_path, &data_doogats, table_name)?;
        }

        self.cleanup_materialized_tables(table_name, schema.as_ref())?;

        Ok(())
    }

    pub(super) fn find_type_data_doogats(&self, table_name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .index
            .sql_conn()
            .prepare("SELECT id, path FROM doogats WHERE type = ?1")?;
        let rows = stmt
            .query_map(params![table_name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    fn cascade_delete_doogats(
        &mut self,
        typedef_path: &str,
        typedef_id: &str,
        data_doogats: &[(String, String)],
        table_name: &str,
    ) -> Result<()> {
        let mut paths: Vec<&str> = vec![typedef_path];
        for (_, path) in data_doogats {
            paths.push(path);
        }
        self.repo
            .delete_files(&paths, &format!("drop table {table_name} cascade"))?;

        self.index.remove_doogat(typedef_id)?;
        for (id, _) in data_doogats {
            self.index.remove_doogat(id)?;
        }
        Ok(())
    }

    fn soft_drop_doogats(
        &mut self,
        typedef_id: &str,
        typedef_path: &str,
        data_doogats: &[(String, String)],
        table_name: &str,
    ) -> Result<()> {
        let mut writes: Vec<(String, String)> = Vec::new();
        for (_, path) in data_doogats {
            let content = self.repo.read_file(path)?;
            let mut parsed = parser::parse(&content, path)?;
            parsed.meta.doogat_type = None;
            writes.push((path.clone(), parser::serialize(&parsed)));
        }

        let write_refs: Vec<(&str, &str)> = writes
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        // Immediate commit — NOT transaction-buffered. DROP TABLE is outside the
        // PRD 00161 task-10 transactional-apply scope: schema-apply never drops a
        // whole type (only columns, via the buffered ALTER path), so this handler
        // is unreachable under an open apply transaction.
        self.repo.commit_batch(
            &write_refs,
            &[typedef_path],
            &format!("drop table {table_name}"),
        )?;

        for (_, path) in data_doogats {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_doogat(&parsed)?;
        }
        self.index.remove_doogat(typedef_id)?;
        Ok(())
    }

    pub(super) fn cleanup_materialized_tables(
        &self,
        table_name: &str,
        schema: Option<&TableSchema>,
    ) -> Result<()> {
        if let Some(schema) = schema {
            for col in &schema.columns {
                if col.references.is_some() {
                    self.index.sql_conn().execute(
                        &format!(
                            "DROP TABLE IF EXISTS \"{table_name}_{col_name}\"",
                            col_name = col.name
                        ),
                        [],
                    )?;
                }
            }
        }

        self.index
            .sql_conn()
            .execute(&format!("DROP TABLE IF EXISTS \"{table_name}\""), [])?;

        Ok(())
    }
}
