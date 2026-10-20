mod alter;
mod create;
mod drop;
mod rename;
mod unique_ops;
mod zone_ops;

pub(super) use unique_ops::parse_quoted_column_list;

use rusqlite::params;
use sqlparser::ast::TableConstraint;

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::{ColumnDef, TableSchema};

use super::builders::schema_from_parsed;
use super::SqlEngine;

/// All file writes and deletes that an `ALTER TABLE foo RENAME TO bar` will
/// perform. Computed up front so the operation can fail validation before
/// touching the working tree.
#[derive(Debug, Default)]
pub(super) struct RenamePlan {
    pub writes: Vec<(String, String)>,
    pub deletes: Vec<String>,
}

/// Git commit message emitted when `CREATE TABLE ... SINGLETON DEFAULT
/// VALUES` runs and the originating node auto-seeds one row in the same
/// commit as the typedef. Exposed as a function so `sync_test` and other
/// callers can assert on the message without depending on the format
/// literal (PRD 00139 cycle-3 #8).
pub fn create_table_with_default_seed_commit_msg(table_name: &str) -> String {
    format!("create table {table_name} with default seed")
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
    pub(super) fn load_typedef_location(&mut self, table_name: &str) -> Result<(String, String)> {
        self.index
            .sql_conn()
            .query_row(
                "SELECT id, path FROM doogats WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| DoogatError::SqlEngine(format!("table not found: {table_name}")))
    }

    pub(crate) fn load_schema(&mut self, table_name: &str) -> Result<TableSchema> {
        let (_id, path) = self.load_typedef_location(table_name)?;
        let content = self.read_typedef_content(&path)?;
        let parsed = parser::parse(&content, &path)?;
        schema_from_parsed(&parsed)
    }

    /// Read a typedef's markdown content with read-your-writes semantics inside
    /// a transaction (PRD 00161 task 10). DDL ops buffer typedef writes instead
    /// of committing them while a transaction is open, so a later op in the same
    /// transaction must see an earlier op's buffered content (the last write to
    /// the path wins) before falling back to the committed git copy. Without
    /// this, two adds to the same type in one apply read the stale pre-buffer
    /// typedef and the second silently overwrites the first.
    fn read_typedef_content(&self, path: &str) -> Result<String> {
        if let Some(ref buf) = self.txn {
            if let Some(write) = buf.writes.iter().rev().find(|w| w.path == path) {
                return Ok(write.content.clone());
            }
        }
        self.repo.read_file(path)
    }
}

/// Extract UNIQUE table constraints into the `unique_together` format.
///
/// Walks both forms:
/// - Table-level `UNIQUE(col1, col2)` (multi-column groups).
/// - Column-level `col TYPE UNIQUE` (single-column groups). sqlparser
///   represents this as `ColumnOption::Unique { is_primary: false, .. }`
///   on the column option list and does not fold it into a TableConstraint,
///   so without this branch column-level UNIQUE silently disappears at
///   typedef registration.
fn extract_unique_constraints(
    constraints: &[TableConstraint],
    columns: &[sqlparser::ast::ColumnDef],
) -> Option<Vec<Vec<String>>> {
    let mut groups: Vec<Vec<String>> = constraints
        .iter()
        .filter_map(|c| match c {
            TableConstraint::Unique { columns, .. } => {
                let cols: Vec<String> = columns.iter().map(|id| id.value.to_lowercase()).collect();
                if cols.is_empty() {
                    None
                } else {
                    Some(cols)
                }
            }
            _ => None,
        })
        .collect();
    for col in columns {
        let col_name = col.name.value.to_lowercase();
        if col_name == "id" || col_name == "type" {
            continue;
        }
        let is_unique = col.options.iter().any(|opt| {
            matches!(
                opt.option,
                sqlparser::ast::ColumnOption::Unique {
                    is_primary: false,
                    ..
                }
            )
        });
        if is_unique {
            let group = vec![col_name];
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
    }
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
