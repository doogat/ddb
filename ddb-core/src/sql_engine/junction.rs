use sqlparser::ast::SetExpr;

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::TableSchema;

use super::helpers::eval_values;
use super::{PendingWrite, SqlEngine, SqlResult};

impl<'a> SqlEngine<'a> {
    /// Collect referenced types that use folder storage (for path-qualified wikilinks).
    pub(super) fn ref_folder_types(
        &self,
        schema: &TableSchema,
    ) -> std::collections::HashSet<String> {
        schema
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref())
            .filter(|ref_table| self.index.type_uses_folder(ref_table, self.repo))
            .cloned()
            .collect()
    }

    pub(super) fn resolve_junction_table(
        &mut self,
        table_name: &str,
    ) -> Result<Option<(String, String)>> {
        // Try each possible split point of `type_col`
        for (i, _) in table_name.match_indices('_') {
            let candidate_type = &table_name[..i];
            let candidate_col = &table_name[i + 1..];
            if candidate_type.is_empty() || candidate_col.is_empty() {
                continue;
            }
            // Check if candidate_type is a known typedef
            if self.load_typedef_location(candidate_type).is_ok() {
                let schema = self.load_schema(candidate_type)?;
                // Check if candidate_col is a REFERENCES column
                if schema
                    .columns
                    .iter()
                    .any(|c| c.name == candidate_col && c.references.is_some())
                {
                    return Ok(Some((
                        candidate_type.to_string(),
                        candidate_col.to_string(),
                    )));
                }
            }
        }
        Ok(None)
    }

    /// Handle INSERT into a junction table by appending reference lines to the
    /// parent doogat and re-indexing.
    pub(super) fn handle_junction_insert(
        &mut self,
        ins: &sqlparser::ast::Insert,
        type_name: &str,
        col_name: &str,
    ) -> Result<SqlResult> {
        let col_names: Vec<String> = ins.columns.iter().map(|c| c.value.to_lowercase()).collect();
        let type_id_col = format!("{type_name}_id");
        let ref_id_col = format!("{col_name}_id");

        let rows = match ins.source.as_ref() {
            Some(query) => match query.body.as_ref() {
                SetExpr::Values(v) => {
                    let mut rows = Vec::with_capacity(v.rows.len());
                    for row in &v.rows {
                        rows.push(eval_values(&self.index.conn, row)?);
                    }
                    rows
                }
                _ => {
                    return Err(DoogatError::SqlEngine(
                        "only VALUES clause supported for junction INSERT".into(),
                    ))
                }
            },
            None => return Err(DoogatError::SqlEngine("missing VALUES clause".into())),
        };

        let schema = self.load_schema(type_name)?;
        let ref_col = schema
            .columns
            .iter()
            .find(|c| c.name == col_name && c.references.is_some())
            .ok_or_else(|| {
                DoogatError::SqlEngine(format!(
                    "column {col_name} not found or not a REFERENCES column"
                ))
            })?;
        let ref_folder_types = self.ref_folder_types(&schema);

        let mut affected = 0;
        for row_values in &rows {
            let parent_id_idx = col_names
                .iter()
                .position(|c| *c == type_id_col)
                .ok_or_else(|| {
                    DoogatError::SqlEngine(format!("missing {type_id_col} column in INSERT"))
                })?;
            let target_id_idx =
                col_names
                    .iter()
                    .position(|c| *c == ref_id_col)
                    .ok_or_else(|| {
                        DoogatError::SqlEngine(format!("missing {ref_id_col} column in INSERT"))
                    })?;

            let parent_id = &row_values[parent_id_idx];
            let target_id = &row_values[target_id_idx];

            // Read parent doogat (txn-aware: picks up buffered writes)
            let path = self.index.resolve_path(parent_id)?;
            let content = self.read_content(&path)?;
            let mut parsed = parser::parse(&content, &path)?;

            // Build the reference line with folder-qualified link if needed
            let link_target = if let Some(ref ref_table) = ref_col.references {
                if ref_folder_types.contains(ref_table) {
                    format!("ddb/{ref_table}/{target_id}.md")
                } else {
                    target_id.clone()
                }
            } else {
                target_id.clone()
            };
            let ref_line = format!("- {}:: [[{}]]", col_name, link_target);

            // Skip if reference line already exists (idempotent, 0 affected)
            if parsed
                .reference_section
                .lines()
                .any(|line| line.trim() == ref_line.trim())
            {
                continue;
            }

            // Append to reference section
            let trimmed = parsed.reference_section.trim_end();
            parsed.reference_section = if trimmed.is_empty() {
                format!("{ref_line}\n")
            } else {
                format!("{trimmed}\n{ref_line}\n")
            };

            // Serialize
            let new_content = parser::serialize(&parsed);

            // Re-index this doogat
            let re_parsed = parser::parse(&new_content, &path)?;
            self.index.index_doogat(&re_parsed)?;
            self.index
                .materialize_single(&schema, parent_id, &re_parsed)?;

            if let Some(ref mut buf) = self.txn {
                buf.writes.push(PendingWrite {
                    path,
                    content: new_content,
                });
            } else {
                self.repo.commit_file(
                    &path,
                    &new_content,
                    &format!("add {col_name} ref {target_id} to {type_name} {parent_id}"),
                )?;
            }

            affected += 1;
        }

        Ok(SqlResult::Affected(affected))
    }

    /// Handle DELETE from a junction table by removing matching reference lines
    /// from the parent doogat and re-indexing.
    pub(super) fn handle_junction_delete(
        &mut self,
        type_name: &str,
        col_name: &str,
        parent_id: &str,
        target_id: &str,
    ) -> Result<SqlResult> {
        let schema = self.load_schema(type_name)?;
        let ref_col = schema
            .columns
            .iter()
            .find(|c| c.name == col_name && c.references.is_some())
            .ok_or_else(|| {
                DoogatError::SqlEngine(format!(
                    "column {col_name} not found or not a REFERENCES column"
                ))
            })?;
        let ref_folder_types = self.ref_folder_types(&schema);

        // Read parent doogat (txn-aware: picks up buffered writes)
        let path = self.index.resolve_path(parent_id)?;
        let content = self.read_content(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        // Build the reference line pattern to remove
        let link_target = if let Some(ref ref_table) = ref_col.references {
            if ref_folder_types.contains(ref_table) {
                format!("ddb/{ref_table}/{target_id}.md")
            } else {
                target_id.to_string()
            }
        } else {
            target_id.to_string()
        };
        let ref_line = format!("- {}:: [[{}]]", col_name, link_target);

        // Remove matching line from reference section
        let old_section = parsed.reference_section.clone();
        let new_lines: Vec<&str> = old_section
            .lines()
            .filter(|line| line.trim() != ref_line.trim())
            .collect();
        let new_section = if new_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", new_lines.join("\n"))
        };

        // Skip commit if nothing changed
        if new_section == old_section {
            return Ok(SqlResult::Affected(0));
        }
        parsed.reference_section = new_section;

        // Serialize
        let new_content = parser::serialize(&parsed);

        // Re-index
        let re_parsed = parser::parse(&new_content, &path)?;
        self.index.index_doogat(&re_parsed)?;
        self.index
            .materialize_single(&schema, parent_id, &re_parsed)?;

        if let Some(ref mut buf) = self.txn {
            buf.writes.push(PendingWrite {
                path,
                content: new_content,
            });
        } else {
            self.repo.commit_file(
                &path,
                &new_content,
                &format!("remove {col_name} ref {target_id} from {type_name} {parent_id}"),
            )?;
        }

        Ok(SqlResult::Affected(1))
    }
}
