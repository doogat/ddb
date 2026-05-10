use rusqlite::params;
use sqlparser::ast::{AlterColumnOperation, AlterTableOperation, ObjectType, TableConstraint};

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::{is_core_column, junction_table_ddl};
use crate::parser;
use crate::types::{ColumnDef, DoogatId, TableSchema, Zone};

use super::builders::{build_typedef_doogat, rename_key_in_doogat, schema_from_parsed};
use super::helpers::{
    data_type_to_string, extract_allowed_values, extract_default, extract_on_delete,
    extract_references, is_not_null, is_numeric_type, is_reserved_table, is_short_string_type,
    unquote_identifier, validate_rename_target_name,
};
use super::{SqlEngine, SqlResult};

/// All file writes and deletes that an `ALTER TABLE foo RENAME TO bar` will
/// perform. Computed up front so the operation can fail validation before
/// touching the working tree.
#[derive(Debug, Default)]
pub(super) struct RenamePlan {
    pub writes: Vec<(String, String)>,
    pub deletes: Vec<String>,
}

/// Apply path-based wikilink rewrites for every `(old_path, new_path)` pair.
/// Each pair is rewritten both with and without the trailing `.md`, covering
/// the two common wikilink target shapes. ID-based links are left alone.
fn apply_path_pair_rewrites(content: &str, path_pairs: &[(String, String)]) -> String {
    let mut out = content.to_string();
    for (old_path, new_path) in path_pairs {
        out = parser::rewrite_links(&out, old_path, new_path);
        let old_no_md = old_path.trim_end_matches(".md");
        let new_no_md = new_path.trim_end_matches(".md");
        if old_no_md != old_path {
            out = parser::rewrite_links(&out, old_no_md, new_no_md);
        }
    }
    out
}

/// Coarse classification of a SQL type literal. Used by `ALTER COLUMN TYPE`
/// to distinguish CHAR from VARCHAR without falling into the trap of
/// treating them as one family. `Other` covers blobs, dates, and any type
/// not listed in PRD 00128.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TypeKind {
    Varchar(u32),
    Char(u32),
    Text,
    Integer,
    Real,
    Boolean,
    Other,
}

impl TypeKind {
    fn classify(uppercased: &str) -> Self {
        if uppercased == "TEXT" {
            return TypeKind::Text;
        }
        if uppercased == "INTEGER" {
            return TypeKind::Integer;
        }
        if uppercased == "REAL" {
            return TypeKind::Real;
        }
        if uppercased == "BOOLEAN" {
            return TypeKind::Boolean;
        }
        if let Some(rest) = uppercased.strip_prefix("VARCHAR(") {
            if let Some(n) = rest.strip_suffix(')').and_then(|s| s.trim().parse().ok()) {
                return TypeKind::Varchar(n);
            }
        }
        if let Some(rest) = uppercased.strip_prefix("CHAR(") {
            if let Some(n) = rest.strip_suffix(')').and_then(|s| s.trim().parse().ok()) {
                return TypeKind::Char(n);
            }
        }
        TypeKind::Other
    }
}

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
            .sql_conn()
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
        let unique_together = extract_unique_constraints(&ct.constraints);
        // PRD 00139 §2: take() the pre-parse SINGLETON marker. `Some(_)`
        // flips the flag on the typedef; the inner bool drives T7's
        // auto-seed (filled in once T7 lands).
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
        // T5 keeps the auto-seed flag local; T7 consumes it after the
        // typedef commit lands. The leading underscore suppresses the
        // unused-binding warning until T7 wires the seed path.
        let _auto_seed = pending_singleton.unwrap_or(false);

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
                let index_name =
                    format!("{}_unique_{}", schema.table_name, cols.join("_"));
                self.index.sql_conn().execute(
                    &format!(
                        "CREATE UNIQUE INDEX IF NOT EXISTS \"{}\" ON \"{}\" ({})",
                        index_name, schema.table_name, col_list
                    ),
                    [],
                )?;
            }
        }

        Ok(())
    }

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
        if (dv == "NEXT" || dv.starts_with("NEXT("))
            && !data_type.eq_ignore_ascii_case("integer")
        {
            return Err(DoogatError::SqlEngine(format!(
                "DEFAULT NEXT is only valid on INTEGER columns, not {data_type}"
            )));
        }
        if dv.starts_with("NEXT(") && dv.ends_with(')') {
            let partition_col = &dv[5..dv.len() - 1];
            let col_exists = schema.columns.iter().any(|c| c.name == partition_col)
                || partition_col == col_name;
            if !col_exists {
                return Err(DoogatError::SqlEngine(format!(
                    "DEFAULT NEXT({partition_col}): column '{partition_col}' not found in table"
                )));
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
                    self.validate_next_default(
                        default_value.as_deref(),
                        &dt,
                        &col_name,
                        &schema,
                    )?;
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
                    if self.handle_alter_column_type(&table_name, &mut schema, &col_name, &new_type)?
                    {
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
        self.index.rematerialize_type(&table_name, self.repo)?;

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
            (TypeKind::Integer, TypeKind::Real)
            | (TypeKind::Real, TypeKind::Integer) => {
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
    fn preflight_numeric(
        &self,
        table_name: &str,
        col_name: &str,
        new_type: &str,
    ) -> Result<()> {
        let sql = format!(
            "SELECT \"{col_name}\" FROM \"{table_name}\" WHERE \"{col_name}\" IS NOT NULL"
        );
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
    pub(super) fn handle_search_key(
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

    /// Validate a `title_template` against the current typedef and referenced
    /// typedefs. Rejects multi-hop paths, malformed identifiers, and dotted
    /// tokens whose `col` is not a REFERENCES column on this type or whose
    /// `field` does not exist on the target type.
    fn validate_title_template(
        &mut self,
        schema: &TableSchema,
        template: &str,
    ) -> Result<()> {
        use super::helpers::parse_title_template;

        let placeholders = parse_title_template(template)?;
        for p in &placeholders {
            let Some(field) = &p.field else {
                continue;
            };
            let col_def = schema.columns.iter().find(|c| c.name == p.col).ok_or_else(|| {
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

    /// `ALTER TABLE foo RENAME TO bar`. Validates inputs, computes a rename
    /// plan covering typedef + folder + REFERENCES + backlinks, commits all
    /// writes in one git commit, then renames the materialized SQLite table
    /// and reindexes affected files. Crash semantics are git's: either the
    /// commit lands or it doesn't. A crash between commit and SQLite
    /// rename is recoverable via `consistency::fix_all`.
    fn handle_rename_table(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        new_name: &str,
    ) -> Result<SqlResult> {
        validate_rename_target_name(new_name)?;

        if self.load_typedef_location(new_name).is_ok() {
            return Err(DoogatError::SqlEngine(format!(
                "typedef already exists: {new_name}"
            )));
        }

        let data_doogats = self.find_type_data_doogats(table_name)?;
        for (id, path) in &data_doogats {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            let actual = parsed.meta.doogat_type.as_deref();
            if actual != Some(table_name) {
                return Err(DoogatError::SqlEngine(format!(
                    "consistency error: doogat {id} at {path} has type {actual:?}, expected {table_name:?}"
                )));
            }
        }

        let old_schema = self.load_schema(table_name)?;

        let plan = self.build_rename_plan(
            table_name,
            typedef_id,
            typedef_path,
            new_name,
            &data_doogats,
        )?;

        let write_refs: Vec<(&str, &str)> = plan
            .writes
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        let delete_refs: Vec<&str> = plan.deletes.iter().map(String::as_str).collect();
        self.repo.commit_batch(
            &write_refs,
            &delete_refs,
            &format!("rename type {table_name} → {new_name}"),
        )?;

        self.cleanup_materialized_tables(table_name, Some(&old_schema))?;

        for (path, _) in &plan.writes {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_doogat(&parsed)?;
        }

        self.index.rematerialize_type(new_name, self.repo)?;

        Ok(SqlResult::Ok(format!(
            "table {table_name} renamed to {new_name}"
        )))
    }

    fn build_rename_plan(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        new_name: &str,
        data_doogats: &[(String, String)],
    ) -> Result<RenamePlan> {
        let mut writes: Vec<(String, String)> = Vec::new();
        let mut deletes: Vec<String> = Vec::new();

        let folder_prefix = format!("ddb/{table_name}/");
        let path_pairs: Vec<(String, String)> = data_doogats
            .iter()
            .filter(|(_, old_path)| old_path.starts_with(&folder_prefix))
            .map(|(id, old_path)| (old_path.clone(), format!("ddb/{new_name}/{id}.md")))
            .collect();

        self.plan_typedef_rewrite(
            table_name,
            typedef_id,
            typedef_path,
            new_name,
            &mut writes,
        )?;

        self.plan_data_doogats_move(
            table_name,
            new_name,
            data_doogats,
            &path_pairs,
            &mut writes,
            &mut deletes,
        )?;

        self.plan_references_rewrite(table_name, typedef_id, new_name, &mut writes)?;

        self.plan_backlinks_rewrite(table_name, &path_pairs, &deletes, &mut writes)?;

        Ok(RenamePlan { writes, deletes })
    }

    fn plan_typedef_rewrite(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        new_name: &str,
        writes: &mut Vec<(String, String)>,
    ) -> Result<()> {
        let mut schema = self.load_schema(table_name)?;
        schema.table_name = new_name.to_string();
        let typedef =
            build_typedef_doogat(&DoogatId(typedef_id.to_string()), &schema);
        writes.push((typedef_path.to_string(), parser::serialize(&typedef)));
        Ok(())
    }

    fn plan_data_doogats_move(
        &self,
        table_name: &str,
        new_name: &str,
        data_doogats: &[(String, String)],
        path_pairs: &[(String, String)],
        writes: &mut Vec<(String, String)>,
        deletes: &mut Vec<String>,
    ) -> Result<()> {
        let folder_prefix = format!("ddb/{table_name}/");
        for (id, old_path) in data_doogats {
            let content = self.repo.read_file(old_path)?;
            let mut parsed = parser::parse(&content, old_path)?;
            parsed.meta.doogat_type = Some(new_name.to_string());
            let serialized = parser::serialize(&parsed);
            let rewritten = apply_path_pair_rewrites(&serialized, path_pairs);
            if old_path.starts_with(&folder_prefix) {
                let new_path = format!("ddb/{new_name}/{id}.md");
                writes.push((new_path, rewritten));
                deletes.push(old_path.clone());
            } else {
                // Flat layout: keep the path, just rewrite content.
                writes.push((old_path.clone(), rewritten));
            }
        }
        Ok(())
    }

    fn plan_references_rewrite(
        &mut self,
        table_name: &str,
        source_typedef_id: &str,
        new_name: &str,
        writes: &mut Vec<(String, String)>,
    ) -> Result<()> {
        let mut stmt = self.index.sql_conn().prepare(
            "SELECT id, path FROM doogats WHERE type = '_typedef' AND id != ?1",
        )?;
        let other_typedefs: Vec<(String, String)> = stmt
            .query_map(params![source_typedef_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        for (other_id, other_path) in other_typedefs {
            let content = self.repo.read_file(&other_path)?;
            let parsed = parser::parse(&content, &other_path)?;
            let mut schema = schema_from_parsed(&parsed)?;
            let mut changed = false;
            for col in schema.columns.iter_mut() {
                if col.references.as_deref() == Some(table_name) {
                    col.references = Some(new_name.to_string());
                    changed = true;
                }
            }
            if changed {
                let rewritten = build_typedef_doogat(&DoogatId(other_id), &schema);
                writes.push((other_path, parser::serialize(&rewritten)));
            }
        }
        Ok(())
    }

    fn plan_backlinks_rewrite(
        &self,
        table_name: &str,
        path_pairs: &[(String, String)],
        deletes: &[String],
        writes: &mut Vec<(String, String)>,
    ) -> Result<()> {
        let folder_prefix = format!("ddb/{table_name}/");
        let mut stmt = self.index.sql_conn().prepare(
            "SELECT DISTINCT z.path FROM _ddb_links l \
             JOIN doogats z ON l.source_id = z.id \
             WHERE l.target_path LIKE ?1 || '%'",
        )?;
        let backlink_paths: Vec<String> = stmt
            .query_map(params![folder_prefix], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        for src_path in backlink_paths {
            if deletes.contains(&src_path) || writes.iter().any(|(p, _)| *p == src_path) {
                continue;
            }
            let original = self.repo.read_file(&src_path)?;
            let rewritten = apply_path_pair_rewrites(&original, path_pairs);
            if rewritten != original {
                writes.push((src_path, rewritten));
            }
        }
        Ok(())
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
        let data_writes =
            self.rewrite_data_doogats_for_rename(&data_doogats, old_name, new_name, &zone)?;

        let mut files: Vec<(String, String)> = Vec::with_capacity(data_writes.len() + 1);
        files.push((typedef_path.to_string(), typedef_content.clone()));
        files.extend(data_writes);

        let file_refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_files(
            &file_refs,
            &format!("alter table {table_name} rename {old_name} to {new_name}"),
        )?;

        self.reindex_after_rename(&typedef_content, typedef_path, &data_doogats, table_name)?;

        Ok(SqlResult::Ok(format!(
            "renamed {old_name} to {new_name} in {table_name}"
        )))
    }

    fn rewrite_data_doogats_for_rename(
        &self,
        data_doogats: &[(String, String)],
        old_name: &str,
        new_name: &str,
        zone: &Zone,
    ) -> Result<Vec<(String, String)>> {
        let mut writes = Vec::with_capacity(data_doogats.len());
        for (_, path) in data_doogats {
            let content = self.repo.read_file(path)?;
            let mut parsed = parser::parse(&content, path)?;
            rename_key_in_doogat(&mut parsed, old_name, new_name, zone);
            writes.push((path.clone(), parser::serialize(&parsed)));
        }
        Ok(writes)
    }

    fn reindex_after_rename(
        &mut self,
        typedef_content: &str,
        typedef_path: &str,
        data_doogats: &[(String, String)],
        table_name: &str,
    ) -> Result<()> {
        let parsed_typedef = parser::parse(typedef_content, typedef_path)?;
        self.index.index_doogat(&parsed_typedef)?;
        for (_, path) in data_doogats {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_doogat(&parsed)?;
        }
        self.index.rematerialize_type(table_name, self.repo)?;
        Ok(())
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
            self.cascade_delete_doogats(
                &typedef_path,
                &typedef_id,
                &data_doogats,
                table_name,
            )?;
        } else {
            self.soft_drop_doogats(
                &typedef_id,
                &typedef_path,
                &data_doogats,
                table_name,
            )?;
        }

        self.cleanup_materialized_tables(table_name, schema.as_ref())?;

        Ok(())
    }

    fn find_type_data_doogats(&self, table_name: &str) -> Result<Vec<(String, String)>> {
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

    fn cleanup_materialized_tables(
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

    pub(super) fn load_typedef_location(
        &mut self,
        table_name: &str,
    ) -> Result<(String, String)> {
        self.index
            .sql_conn()
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

/// Extract UNIQUE table constraints into the `unique_together` format.
fn extract_unique_constraints(
    constraints: &[TableConstraint],
) -> Option<Vec<Vec<String>>> {
    let groups: Vec<Vec<String>> = constraints
        .iter()
        .filter_map(|c| match c {
            TableConstraint::Unique { columns, .. } => {
                let cols: Vec<String> =
                    columns.iter().map(|id| id.value.to_lowercase()).collect();
                if cols.is_empty() {
                    None
                } else {
                    Some(cols)
                }
            }
            _ => None,
        })
        .collect();
    if groups.is_empty() {
        None
    } else {
        Some(groups)
    }
}

fn validate_next_partition_refs(columns: &[ColumnDef]) -> Result<()> {
    let col_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    for col in columns {
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
    Ok(())
}
