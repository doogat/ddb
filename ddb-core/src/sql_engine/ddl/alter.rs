use rusqlite::params;
use sqlparser::ast::{AlterColumnOperation, AlterTableOperation};

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::is_core_column;
use crate::parser;
use crate::types::{ColumnDef, DoogatId, TableSchema, Zone};

use crate::sql_engine::builders::build_typedef_doogat;
use crate::sql_engine::helpers::{
    data_type_to_string, extract_allowed_values, extract_default, extract_on_delete,
    extract_references, is_not_null, is_numeric_type, is_short_string_type, unquote_identifier,
};
use crate::sql_engine::{PendingWrite, SqlEngine, SqlResult};

use super::TypeKind;

impl<'a> SqlEngine<'a> {
    /// Validate a DEFAULT NEXT or NEXT(col) value on a column definition.
    fn validate_next_default(
        &self,
        default_value: Option<&str>,
        data_type: &str,
        col_name: &str,
        schema: &TableSchema,
    ) -> Result<()> {
        let dv = match default_value {
            Some(dv) => dv,
            None => return Ok(()),
        };
        if (dv == "NEXT" || dv.starts_with("NEXT(")) && !data_type.eq_ignore_ascii_case("integer") {
            return Err(DoogatError::SqlEngine(format!(
                "DEFAULT NEXT is only valid on INTEGER columns, not {data_type}"
            )));
        }
        if dv.starts_with("NEXT(") && dv.ends_with(')') {
            let partition_col = &dv[5..dv.len() - 1];
            let col_exists =
                schema.columns.iter().any(|c| c.name == partition_col) || partition_col == col_name;
            if !col_exists {
                return Err(DoogatError::SqlEngine(format!(
                    "DEFAULT NEXT({partition_col}): column '{partition_col}' not found in table"
                )));
            }
        }
        Ok(())
    }

    pub(in crate::sql_engine) fn handle_alter_table(
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
                    self.validate_next_default(default_value.as_deref(), &dt, &col_name, &schema)?;
                    let zone = if refs.is_some() {
                        Some(Zone::Reference)
                    } else if is_numeric_type(&dt) || is_short_string_type(&column_def.data_type) {
                        Some(Zone::Frontmatter)
                    } else {
                        Some(Zone::Body)
                    };
                    let on_delete = extract_on_delete(&column_def.options)?;
                    schema.columns.push(ColumnDef {
                        name: col_name,
                        data_type: dt,
                        zone,
                        required: is_not_null(&column_def.options),
                        search_boost: None,
                        references: refs,
                        allowed_values: extract_allowed_values(&column_def.data_type),
                        default_value,
                        on_delete,
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
                AlterTableOperation::AlterColumn {
                    column_name,
                    op: AlterColumnOperation::SetDataType { data_type, .. },
                } => {
                    let col_name = column_name.value.to_lowercase();
                    let new_type = data_type_to_string(data_type);
                    if self.handle_alter_column_type(
                        &table_name,
                        &mut schema,
                        &col_name,
                        &new_type,
                    )? {
                        // Idempotent no-op: skip persistence and rematerialize.
                        return Ok(SqlResult::Ok(format!("table {table_name} altered")));
                    }
                }
                AlterTableOperation::RenameTable {
                    table_name: new_table_name,
                } => {
                    return self.handle_rename_table(
                        &table_name,
                        &typedef_id,
                        &typedef_path,
                        &unquote_identifier(&new_table_name.to_string()),
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
        self.rematerialize_or_defer(&table_name)?;

        Ok(SqlResult::Ok(format!("table {table_name} altered")))
    }

    /// Handle `ALTER TABLE t ALTER COLUMN c SET DATA TYPE new_type`.
    ///
    /// Mutates `schema.columns[i].data_type` in place when the conversion is
    /// allowed, otherwise returns `Err`. Returns `Ok(true)` for the idempotent
    /// no-op case (new type equals old type), signalling the caller to skip
    /// typedef persistence and rematerialize entirely. Returns `Ok(false)`
    /// for a successful change that still needs to be persisted.
    ///
    /// Supported conversions (v1):
    ///   - `VARCHAR(N) -> VARCHAR(M)` where `M > N` — metadata-only
    ///   - `CHAR(N) -> CHAR(M)` where `M > N` — metadata-only
    ///   - `VARCHAR(N)` / `CHAR(N)` -> `TEXT` — metadata-only
    ///   - `VARCHAR(N) -> VARCHAR(M)` where `M < N` — pre-flight scan
    ///   - `CHAR(N) -> CHAR(M)` where `M < N` — pre-flight scan
    ///   - `TEXT -> VARCHAR(N)` — pre-flight scan
    ///   - `INTEGER <-> REAL` — pre-flight scan on existing values
    ///
    /// CHAR ↔ VARCHAR cross-family conversions are rejected (semantic
    /// mismatch on padding/trimming behavior).
    /// REFERENCES columns only allow widening within the same family or to TEXT.
    fn handle_alter_column_type(
        &self,
        table_name: &str,
        schema: &mut TableSchema,
        col_name: &str,
        new_type: &str,
    ) -> Result<bool> {
        // Core columns (id, type, title, date, etc.) are materialized as TEXT
        // regardless of their declared type. Allowing a type change on the
        // typedef would persist metadata that contradicts materialization.
        if is_core_column(col_name) {
            return Err(DoogatError::SqlEngine(format!(
                "cannot alter {table_name}.{col_name}: {col_name} is a core column managed by ddb"
            )));
        }

        let idx = schema
            .columns
            .iter()
            .position(|c| c.name == col_name)
            .ok_or_else(|| DoogatError::SqlEngine(format!("column not found: {col_name}")))?;

        let old_type = schema.columns[idx].data_type.clone();
        let is_reference = schema.columns[idx].references.is_some();

        // Idempotent case: same type, caller skips persistence.
        if old_type.eq_ignore_ascii_case(new_type) {
            return Ok(true);
        }

        let old_up = old_type.to_uppercase();
        let new_up = new_type.to_uppercase();
        let old_kind = TypeKind::classify(&old_up);
        let new_kind = TypeKind::classify(&new_up);

        let unsupported = || {
            DoogatError::SqlEngine(format!(
                "cannot alter {table_name}.{col_name}: conversion from {old_type} to {new_type} is not supported"
            ))
        };

        if matches!(old_kind, TypeKind::Boolean) || matches!(new_kind, TypeKind::Boolean) {
            return Err(unsupported());
        }

        // Reject CHAR <-> VARCHAR cross-family explicitly. Same-length is an
        // identity transition that the idempotent guard above already covers.
        if matches!(
            (&old_kind, &new_kind),
            (TypeKind::Char(_), TypeKind::Varchar(_)) | (TypeKind::Varchar(_), TypeKind::Char(_))
        ) {
            return Err(unsupported());
        }

        let is_widening = match (&old_kind, &new_kind) {
            (TypeKind::Varchar(a), TypeKind::Varchar(b)) => b > a,
            (TypeKind::Char(a), TypeKind::Char(b)) => b > a,
            (TypeKind::Varchar(_) | TypeKind::Char(_), TypeKind::Text) => true,
            _ => false,
        };

        if is_reference && !is_widening {
            return Err(DoogatError::SqlEngine(format!(
                "cannot alter {table_name}.{col_name}: REFERENCES column only supports widening within the same family or to TEXT"
            )));
        }

        let metadata_only = match (&old_kind, &new_kind) {
            (TypeKind::Varchar(a), TypeKind::Varchar(b)) if b >= a => true,
            (TypeKind::Char(a), TypeKind::Char(b)) if b >= a => true,
            (TypeKind::Varchar(_) | TypeKind::Char(_), TypeKind::Text) => true,
            _ => false,
        };

        if metadata_only {
            schema.columns[idx].data_type = new_up;
            return Ok(false);
        }

        // Pre-flight scans for lossy conversions.
        match (&old_kind, &new_kind) {
            (TypeKind::Varchar(_), TypeKind::Varchar(_))
            | (TypeKind::Char(_), TypeKind::Char(_))
            | (TypeKind::Text, TypeKind::Varchar(_))
            | (TypeKind::Text, TypeKind::Char(_)) => {
                self.preflight_narrow_string(table_name, col_name, &new_kind, &new_up)?;
                schema.columns[idx].data_type = new_up;
                Ok(false)
            }
            (TypeKind::Integer, TypeKind::Real) | (TypeKind::Real, TypeKind::Integer) => {
                self.preflight_numeric(table_name, col_name, &new_up)?;
                schema.columns[idx].data_type = new_up;
                Ok(false)
            }
            _ => Err(unsupported()),
        }
    }

    /// Reject a VARCHAR/CHAR narrowing when any existing row exceeds the new
    /// length. NULL rows are allowed through. The error message uses the
    /// caller-supplied `new_type` literal so CHAR narrowing reports `CHAR(N)`
    /// rather than the family-collapsed `VARCHAR(N)`.
    fn preflight_narrow_string(
        &self,
        table_name: &str,
        col_name: &str,
        new_kind: &TypeKind,
        new_type_literal: &str,
    ) -> Result<()> {
        let max_len = match new_kind {
            TypeKind::Varchar(n) | TypeKind::Char(n) => *n,
            _ => unreachable!("preflight_narrow_string called with non-string TypeKind"),
        };
        let sql = format!(
            "SELECT COUNT(*) FROM \"{table_name}\" WHERE \"{col_name}\" IS NOT NULL AND LENGTH(\"{col_name}\") > ?1"
        );
        let count: i64 = self
            .index
            .sql_conn()
            .query_row(&sql, params![max_len as i64], |row| row.get(0))
            .map_err(|e| {
                DoogatError::SqlEngine(format!(
                    "preflight scan failed on {table_name}.{col_name}: {e}"
                ))
            })?;
        if count > 0 {
            return Err(DoogatError::SqlEngine(format!(
                "cannot narrow {table_name}.{col_name} to {new_type_literal}: {count} existing rows exceed limit"
            )));
        }
        Ok(())
    }

    /// Reject an INTEGER↔REAL cross-conversion when any existing value cannot
    /// round-trip through the new type. NULL rows are allowed through.
    fn preflight_numeric(&self, table_name: &str, col_name: &str, new_type: &str) -> Result<()> {
        let sql =
            format!("SELECT \"{col_name}\" FROM \"{table_name}\" WHERE \"{col_name}\" IS NOT NULL");
        let conn = self.index.sql_conn();
        let mut stmt = conn.prepare(&sql).map_err(|e| {
            DoogatError::SqlEngine(format!(
                "preflight scan failed on {table_name}.{col_name}: {e}"
            ))
        })?;
        let rows = stmt
            .query_map([], |row| row.get::<_, rusqlite::types::Value>(0))
            .map_err(|e| {
                DoogatError::SqlEngine(format!(
                    "preflight scan failed on {table_name}.{col_name}: {e}"
                ))
            })?;

        let mut fail_count: u64 = 0;
        for row in rows {
            let value = row.map_err(|e| {
                DoogatError::SqlEngine(format!(
                    "preflight scan failed on {table_name}.{col_name}: {e}"
                ))
            })?;
            let ok = match (&value, new_type) {
                (rusqlite::types::Value::Null, _) => true,
                (rusqlite::types::Value::Integer(_), "INTEGER") => true,
                (rusqlite::types::Value::Integer(_), "REAL") => true,
                (rusqlite::types::Value::Real(f), "INTEGER") => f.fract() == 0.0,
                (rusqlite::types::Value::Real(_), "REAL") => true,
                (rusqlite::types::Value::Text(s), "INTEGER") => s.parse::<i64>().is_ok(),
                (rusqlite::types::Value::Text(s), "REAL") => s.parse::<f64>().is_ok(),
                _ => false,
            };
            if !ok {
                fail_count += 1;
            }
        }
        if fail_count > 0 {
            return Err(DoogatError::SqlEngine(format!(
                "cannot convert {table_name}.{col_name} to {new_type}: {fail_count} existing rows are not valid {new_type}"
            )));
        }
        Ok(())
    }

    /// Serialize a modified TableSchema back to its typedef doogat, commit to
    /// Git (or buffer the write inside a transaction), and re-index.
    ///
    /// Inside an open transaction the git write is buffered into the
    /// `TransactionBuffer` (mirroring the row-write path in `dml.rs`) and
    /// flushed as one `commit_batch` on COMMIT, so a multi-op schema apply is
    /// atomic. The `index_doogat` call always runs under the SAVEPOINT so a
    /// later op's `load_typedef_location` lookup finds the type.
    pub(super) fn update_typedef(&mut self, table_name: &str, schema: &TableSchema) -> Result<()> {
        let (typedef_id, typedef_path) = self.load_typedef_location(table_name)?;
        let id = DoogatId(typedef_id);
        let schema_doogat = build_typedef_doogat(&id, schema);
        let content = parser::serialize(&schema_doogat);
        let parsed = parser::parse(&content, &typedef_path)?;
        if let Some(ref mut buf) = self.txn {
            buf.writes.push(PendingWrite {
                path: typedef_path,
                content,
            });
        } else {
            self.repo
                .commit_file(&typedef_path, &content, &format!("alter table {table_name}"))?;
        }
        self.index.index_doogat(&parsed)?;
        Ok(())
    }

    /// Rebuild a type's materialized table now, or defer it to COMMIT when a
    /// transaction is open (PRD 00161 task 10). `rematerialize_type` reads the
    /// typedef and row content from git and is blind to buffered typedef
    /// writes, so inside a transaction we record the type and let
    /// `handle_commit` rematerialize it after flushing the writes to git.
    pub(super) fn rematerialize_or_defer(&mut self, table_name: &str) -> Result<()> {
        if let Some(ref mut buf) = self.txn {
            if !buf.rematerialize.iter().any(|t| t == table_name) {
                buf.rematerialize.push(table_name.to_string());
            }
            Ok(())
        } else {
            self.index.rematerialize_type(table_name, self.repo)
        }
    }
}
