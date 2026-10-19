use rusqlite::params;

use crate::error::Result;
use crate::traits::DoogatSource;
use crate::types::ParsedDoogat;

use super::super::Index;
use super::schema_helpers::{
    check_cross_zone_duplicates, check_required_fields, collect_zone_columns,
    finalize_schema_columns,
};

impl Index {
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
        // Warn on row-decode failure instead of dropping the path silently
        // (PRD 00140); the warn arm requires a corrupt index to reach.
        let paths: Vec<String> = stmt
            .query_map(params![type_name], |row| row.get(0))?
            .filter_map(|r| match r {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, type_name, "infer_schema: path row decode failed; row skipped");
                    None
                }
            })
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
            Err(e) => {
                tracing::warn!(error = %e, "collect_consistency_warnings: listing doogats failed, returning partial warnings");
                return warnings;
            }
        };

        let typedef_schemas = self.load_all_typedefs(repo);

        for path in &paths {
            let content = match repo.read_file(path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, path, "collect_consistency_warnings: file read failed, skipped");
                    continue;
                }
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
            Err(e) => {
                tracing::warn!(error = %e, "load_all_typedefs: typedef path query prepare failed, no typedefs loaded");
                return schemas;
            }
        };

        let paths: Vec<String> = match stmt.query_map([], |row| row.get(0)) {
            Ok(rows) => rows
                .filter_map(|r| match r {
                    Ok(path) => Some(path),
                    Err(e) => {
                        tracing::warn!(error = %e, "load_all_typedefs: typedef path row decode failed, skipped");
                        None
                    }
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "load_all_typedefs: typedef path query failed, no typedefs loaded");
                Vec::new()
            }
        };

        for path in &paths {
            match repo.read_file(path) {
                Err(e) => {
                    tracing::warn!(error = %e, path, "load_all_typedefs: typedef file read failed, skipped");
                }
                Ok(content) => match crate::parser::parse(&content, path) {
                    Err(e) => {
                        tracing::warn!(error = %e, path, "load_all_typedefs: typedef parse failed, skipped");
                    }
                    Ok(parsed) => match schema_from_parsed(&parsed) {
                        Err(e) => {
                            tracing::warn!(error = %e, path, "load_all_typedefs: schema extraction failed, skipped");
                        }
                        Ok(schema) => {
                            schemas.insert(schema.table_name.clone(), schema);
                        }
                    },
                },
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
        let schemas = super::super::cascade::load_schemas(&self.conn, |path| repo.read_file(path))?;
        super::super::cascade::check_restrict(&self.conn, &schemas, deleted_id)
    }

    /// Collect CASCADE children for the shared service/SQL delete planner.
    pub(crate) fn collect_cascade_children(
        &self,
        repo: &dyn DoogatSource,
        deleted_id: &str,
    ) -> Result<Vec<(String, String)>> {
        let schemas = super::super::cascade::load_schemas(&self.conn, |path| repo.read_file(path))?;
        super::super::cascade::collect_children(&self.conn, &schemas, deleted_id)
    }

    /// Load typedef schemas from pre-parsed doogats (no git reads).
    pub(super) fn load_all_typedefs_from(
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
}
