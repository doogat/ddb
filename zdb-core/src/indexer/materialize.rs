use rusqlite::params;

use crate::error::Result;
use crate::traits::ZettelSource;
use crate::types::ParsedZettel;

use super::Index;

impl Index {
    /// Drop and recreate a materialized SQLite table from a schema.
    fn drop_and_create_materialized_table(&self, schema: &crate::types::TableSchema) -> Result<()> {
        self.conn.execute(
            &format!("DROP TABLE IF EXISTS \"{}\"", schema.table_name),
            [],
        )?;

        let mut col_defs = vec!["id TEXT PRIMARY KEY".to_string()];
        for col in &schema.columns {
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
        self.conn.execute(
            &format!(
                "CREATE TABLE \"{}\" ({})",
                schema.table_name,
                col_defs.join(", ")
            ),
            [],
        )?;
        Ok(())
    }

    /// Populate a materialized table with data zettels of the given type.
    fn populate_materialized_table(
        &self,
        schema: &crate::types::TableSchema,
        type_name: &str,
        repo: &(impl ZettelSource + ?Sized),
    ) -> Result<()> {
        let mut data_stmt = self
            .conn
            .prepare("SELECT id, path FROM zettels WHERE type = ?1")?;
        let data_zettels: Vec<(String, String)> = data_stmt
            .query_map(params![type_name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        for (zettel_id, zettel_path) in &data_zettels {
            let zettel_content = repo.read_file(zettel_path)?;
            let zettel_parsed = crate::parser::parse(&zettel_content, zettel_path)?;
            self.materialize_row(schema, zettel_id, &zettel_parsed)?;
        }
        Ok(())
    }

    /// Rematerialize a single type's SQLite table.
    /// Loads typedef (if any), infers schema from data, merges, drops/creates table, populates rows.
    pub fn rematerialize_type(
        &self,
        type_name: &str,
        repo: &(impl ZettelSource + ?Sized),
    ) -> Result<()> {
        use crate::sql_engine::schema_from_parsed;

        // Load typedef if exists
        let typedef: Option<crate::types::TableSchema> = {
            let mut stmt = self
                .conn
                .prepare("SELECT path FROM zettels WHERE type = '_typedef' AND title = ?1")?;
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

    /// Materialize SQLite tables for all typed zettels using merged schemas.
    /// Returns (tables_materialized, types_inferred).
    pub fn materialize_all_types(&self, repo: &impl ZettelSource) -> Result<(usize, Vec<String>)> {
        let mut tables_materialized = 0;
        let mut types_inferred = Vec::new();

        // Load explicit _typedef schemas
        let typedef_schemas = self.load_all_typedefs(repo);

        // Find all distinct types (excluding _typedef and empty)
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT type FROM zettels WHERE type != '_typedef' AND type != '' AND type IS NOT NULL",
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

        // Also materialize typedef-only types with no data zettels
        for (type_name, schema) in &typedef_schemas {
            if !type_names.contains(type_name) && !schema.columns.is_empty() {
                self.drop_and_create_materialized_table(schema)?;
                tables_materialized += 1;
            }
        }

        Ok((tables_materialized, types_inferred))
    }

    /// Infer a TableSchema from pre-parsed zettels (no git reads).
    pub fn infer_schema_from(
        type_name: &str,
        zettels: &[ParsedZettel],
    ) -> crate::types::TableSchema {
        use crate::types::{ColumnDef, TableSchema, Zone};
        use std::collections::HashMap;

        let mut columns: HashMap<String, (Zone, Vec<String>)> = HashMap::new();

        for parsed in zettels
            .iter()
            .filter(|z| z.meta.zettel_type.as_deref() == Some(type_name))
        {
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
        }
    }

    /// Populate a materialized table from pre-parsed zettels (no git reads).
    fn populate_materialized_table_from(
        &self,
        schema: &crate::types::TableSchema,
        zettels: &[ParsedZettel],
    ) -> Result<()> {
        let type_name = &schema.table_name;
        for zettel in zettels
            .iter()
            .filter(|z| z.meta.zettel_type.as_deref() == Some(type_name.as_str()))
        {
            let id = zettel.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
            self.materialize_row(schema, id, zettel)?;
        }
        Ok(())
    }

    /// Materialize all typed tables from pre-parsed zettels (no git reads).
    pub fn materialize_all_types_from(
        &self,
        zettels: &[ParsedZettel],
    ) -> Result<(usize, Vec<String>)> {
        let mut tables_materialized = 0;
        let mut types_inferred = Vec::new();

        let typedef_schemas = Self::load_all_typedefs_from(zettels);

        // Find distinct types from the pre-parsed data
        let type_names: Vec<String> = zettels
            .iter()
            .filter_map(|z| z.meta.zettel_type.as_deref())
            .filter(|t| !t.is_empty() && *t != "_typedef")
            .map(|t| t.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        for type_name in &type_names {
            let typedef = typedef_schemas.get(type_name.as_str()).cloned();

            let inferred = Self::infer_schema_from(type_name, zettels);
            let schema = Self::merge_schemas(typedef.clone(), inferred);

            if schema.columns.is_empty() {
                continue;
            }

            if typedef.is_none() {
                // Type inference is tracked in types_inferred and returned to caller
                types_inferred.push(type_name.clone());
            }

            self.drop_and_create_materialized_table(&schema)?;
            self.populate_materialized_table_from(&schema, zettels)?;
            tables_materialized += 1;
        }

        // Also materialize typedef-only types with no data zettels
        for (type_name, schema) in &typedef_schemas {
            if !type_names.contains(type_name) && !schema.columns.is_empty() {
                self.drop_and_create_materialized_table(schema)?;
                tables_materialized += 1;
            }
        }

        Ok((tables_materialized, types_inferred))
    }

    /// Infer a TableSchema for a type by scanning all data zettels of that type.
    pub fn infer_schema(
        &self,
        type_name: &str,
        repo: &(impl ZettelSource + ?Sized),
    ) -> Result<crate::types::TableSchema> {
        use crate::types::{ColumnDef, TableSchema, Zone};
        use std::collections::HashMap;

        // Query all zettels of this type
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM zettels WHERE type = ?1")?;
        let paths: Vec<String> = stmt
            .query_map(params![type_name], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        // Track columns: name -> (zone, data_types_seen)
        let mut columns: HashMap<String, (Zone, Vec<String>)> = HashMap::new();

        for path in &paths {
            let content = repo.read_file(path)?;
            let parsed = crate::parser::parse(&content, path)?;

            // Frontmatter extra keys → frontmatter columns
            // Normalize to lowercase — SQLite column names are case-insensitive
            for (key, value) in &parsed.meta.extra {
                let inferred_type = infer_yaml_type(value);
                columns
                    .entry(key.to_lowercase())
                    .or_insert_with(|| (Zone::Frontmatter, Vec::new()))
                    .1
                    .push(inferred_type);
            }

            // Body headings → body TEXT columns (from parsed sections)
            for section in &parsed.sections {
                if section.level > 0 {
                    columns
                        .entry(section.heading.to_lowercase())
                        .or_insert_with(|| (Zone::Body, vec!["TEXT".to_string()]));
                }
            }

            // Reference fields → reference columns
            for field in &parsed.inline_fields {
                if field.zone == Zone::Reference {
                    let entry = columns
                        .entry(field.key.to_lowercase())
                        .or_insert_with(|| (Zone::Reference, Vec::new()));
                    entry.1.push("TEXT".to_string());
                }
            }
        }

        // Build final columns with type widening
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
                }
            })
            .collect();

        // Sort columns for deterministic output
        cols.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(TableSchema {
            table_name: type_name.to_string(),
            columns: cols,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
        })
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
        repo: &impl ZettelSource,
    ) -> Vec<crate::types::ConsistencyWarning> {
        use crate::types::ConsistencyWarning;

        let mut warnings = Vec::new();

        let paths = match repo.list_zettels() {
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
                    // Check for cross-zone duplicate keys
                    let mut seen_keys: std::collections::HashMap<String, &str> =
                        std::collections::HashMap::new();

                    for key in parsed.meta.extra.keys() {
                        seen_keys.insert(key.clone(), "frontmatter");
                    }

                    for field in &parsed.inline_fields {
                        if field.zone == crate::types::Zone::Reference {
                            if let Some(&other_zone) = seen_keys.get(&field.key) {
                                if other_zone != "reference" {
                                    warnings.push(ConsistencyWarning::CrossZoneDuplicate {
                                        path: path.clone(),
                                        key: field.key.clone(),
                                    });
                                }
                            }
                        }
                    }

                    // Check missing required fields
                    if let Some(type_name) = &parsed.meta.zettel_type {
                        if let Some(schema) = typedef_schemas.get(type_name.as_str()) {
                            for col in &schema.columns {
                                if col.required {
                                    let has_value = match col
                                        .zone
                                        .as_ref()
                                        .unwrap_or(&crate::types::Zone::Frontmatter)
                                    {
                                        crate::types::Zone::Frontmatter => {
                                            parsed.meta.extra.contains_key(&col.name)
                                        }
                                        crate::types::Zone::Reference => {
                                            parsed.inline_fields.iter().any(|f| f.key == col.name)
                                        }
                                        crate::types::Zone::Body => {
                                            parsed.body.contains(&format!("## {}", col.name))
                                        }
                                    };
                                    if !has_value {
                                        warnings.push(ConsistencyWarning::MissingRequired {
                                            path: path.clone(),
                                            type_name: type_name.clone(),
                                            field: col.name.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
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
        repo: &impl ZettelSource,
    ) -> std::collections::HashMap<String, crate::types::TableSchema> {
        use crate::sql_engine::schema_from_parsed;

        let mut schemas = std::collections::HashMap::new();

        let mut stmt = match self
            .conn
            .prepare("SELECT path FROM zettels WHERE type = '_typedef'")
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

    /// Load typedef schemas from pre-parsed zettels (no git reads).
    fn load_all_typedefs_from(
        zettels: &[ParsedZettel],
    ) -> std::collections::HashMap<String, crate::types::TableSchema> {
        use crate::sql_engine::schema_from_parsed;

        let mut schemas = std::collections::HashMap::new();
        for z in zettels
            .iter()
            .filter(|z| z.meta.zettel_type.as_deref() == Some("_typedef"))
        {
            if let Ok(schema) = schema_from_parsed(z) {
                schemas.insert(schema.table_name.clone(), schema);
            }
        }
        schemas
    }

    /// Insert a single data zettel's values into a materialized table.
    fn materialize_row(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        zettel: &crate::types::ParsedZettel,
    ) -> Result<()> {
        let mut col_names = vec!["id".to_string()];
        let mut placeholders = vec!["?1".to_string()];
        let mut vals: Vec<Option<String>> = vec![Some(id.to_string())];

        for (i, col) in schema.columns.iter().enumerate() {
            col_names.push(format!("\"{}\"", col.name));
            placeholders.push(format!("?{}", i + 2));
            let val = extract_column_value(zettel, col);
            vals.push(if val.is_empty() { None } else { Some(val) });
        }

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
        Ok(())
    }
}

/// Extract a column value from a parsed zettel according to zone mapping.
fn extract_column_value(
    zettel: &crate::types::ParsedZettel,
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
            for field in &zettel.inline_fields {
                if field.key == col.name {
                    let val = field.value.trim();
                    let val = val.strip_prefix("[[").unwrap_or(val);
                    let val = val.strip_suffix("]]").unwrap_or(val);
                    let val = val.split('|').next().unwrap_or(val);
                    return val.to_string();
                }
            }
            String::new()
        }
        Zone::Frontmatter => {
            // Use path navigation for dot/bracket names, flat lookup otherwise
            let val = if col.name.contains('.') || col.name.contains('[') {
                crate::types::get_path_in_map(&zettel.meta.extra, &col.name).ok()
            } else {
                zettel.meta.extra.get(&col.name)
            };
            val.map(|v| match v {
                crate::types::Value::Number(n) => n.to_string(),
                crate::types::Value::Bool(b) => b.to_string(),
                crate::types::Value::String(s) => s.clone(),
                _ => format!("{v:?}"),
            })
            .unwrap_or_default()
        }
        Zone::Body => zettel
            .sections
            .iter()
            .find(|s| s.level > 0 && s.heading.eq_ignore_ascii_case(&col.name))
            .map(|s| s.content.trim().to_string())
            .unwrap_or_default(),
    }
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
