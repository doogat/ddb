use rusqlite::params;
use sqlparser::ast::Expr;

use crate::cascade_delete::{self, CascadeNode};
use crate::error::Result;
use crate::indexer::{cascade, escape_sql_ident, with_savepoint};

use super::{PendingDelete, PendingWrite, SqlEngine, SqlResult};

impl SqlEngine<'_> {
    pub(super) fn delete_single_row(&mut self, table: &str, id: &str) -> Result<SqlResult> {
        self.delete_rows(&[id.to_string()], &format!("delete from {table} {id}"))?;
        Ok(SqlResult::Affected(1))
    }

    pub(super) fn delete_bulk_rows(
        &mut self,
        table: &str,
        selection: &Option<Expr>,
    ) -> Result<SqlResult> {
        let matches = self.resolve_matching_ids(table, selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }
        let roots: Vec<String> = matches.into_iter().map(|(id, _)| id).collect();
        self.delete_rows(&roots, &format!("delete {} rows from {table}", roots.len()))?;
        // SQL counts directly matched rows, excluding cascade descendants.
        Ok(SqlResult::Affected(roots.len()))
    }

    fn delete_rows(&mut self, roots: &[String], message: &str) -> Result<()> {
        let conn = self.index.sql_conn();
        let schemas = cascade::load_schemas(conn, |path| self.read_content(path))?;
        let plan = cascade_delete::plan(roots, |id| {
            let path = self.index.resolve_path(id)?;
            cascade::check_restrict(conn, &schemas, id)?;
            let children = cascade::collect_children(conn, &schemas, id)?;
            Ok(CascadeNode { path, children })
        })?;
        let edits =
            cascade_delete::reference_edits(self.index, &plan, |path| self.read_content(path))?;

        // A failure after deleting an earlier node must restore all index
        // changes. This savepoint nests inside an existing SQL transaction.
        with_savepoint(conn, "cascade_delete", || {
            for (id, _) in &plan {
                let doogat_type: Option<String> = conn.query_row(
                    "SELECT type FROM doogats WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )?;
                let table = doogat_type
                    .as_deref()
                    .filter(|t| !t.is_empty() && *t != "_typedef");
                if let Some(table) = table {
                    let owner = schemas.get(table).ok_or_else(|| {
                        crate::error::DoogatError::SqlEngine(format!(
                            "cascade delete: cannot load owner schema '{table}'"
                        ))
                    })?;
                    crate::indexer::delete_junction_rows_for_cascade(
                        conn,
                        &schemas,
                        Some(owner),
                        table,
                        id,
                    )?;
                }
                self.index.remove_doogat(id)?;
                if let Some(table) = table {
                    conn.execute(
                        &format!("DELETE FROM \"{}\" WHERE id = ?1", escape_sql_ident(table)),
                        params![id],
                    )?;
                }
            }
            for edit in &edits {
                self.index.index_doogat(&edit.parsed)?;
                if let Some(schema) = edit
                    .parsed
                    .meta
                    .doogat_type
                    .as_ref()
                    .and_then(|t| schemas.get(t))
                {
                    self.index
                        .materialize_single(schema, &edit.id, &edit.parsed)?;
                }
            }
            if self.txn.is_none() {
                let writes: Vec<(&str, &str)> = edits
                    .iter()
                    .map(|e| (e.parsed.path.as_str(), e.content.as_str()))
                    .collect();
                let deletes: Vec<&str> = plan.iter().map(|(_, path)| path.as_str()).collect();
                self.repo.commit_batch(&writes, &deletes, message)?;
            }
            Ok(())
        })?;
        // Append only after every fallible operation succeeds. A failed
        // DELETE preserves any writes already buffered by its caller.
        if let Some(buf) = &mut self.txn {
            buf.deletes
                .extend(plan.into_iter().map(|(id, path)| PendingDelete {
                    path,
                    doogat_id: id,
                }));
            buf.writes
                .extend(edits.into_iter().map(|edit| PendingWrite {
                    path: edit.parsed.path,
                    content: edit.content,
                }));
        }
        Ok(())
    }
}
