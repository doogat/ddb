use rusqlite::params;

use crate::cascade_delete::{self, CascadeNode};
use crate::error::Result;
use crate::indexer::{escape_sql_ident, with_savepoint};
use crate::traits::{GitBackend, IndexPort};

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Delete a doogat and its CASCADE descendants in one Git commit.
    /// Returns broken backlinks `(source_id, source_path)` for the root.
    /// RESTRICT and cycle checks finish before any index or Git writes.
    pub fn delete_doogat(&self, id: &str, message: &str) -> Result<Vec<(String, String)>> {
        self.ensure_fresh()?;
        let plan = cascade_delete::plan(&[id.to_string()], |id| {
            let path = self.index.resolve_path(id)?;
            self.index.check_restrict_blocks_delete(&self.repo, id)?;
            let children = self.index.collect_cascade_children(&self.repo, id)?;
            Ok(CascadeNode { path, children })
        })?;
        self.execute_delete_plan(plan, id, message)
    }

    fn execute_delete_plan(
        &self,
        plan: Vec<(String, String)>,
        root_id: &str,
        message: &str,
    ) -> Result<Vec<(String, String)>> {
        let broken = self.index.backlinking_doogat_paths(root_id)?;
        let edits =
            cascade_delete::reference_edits(&self.index, &plan, |path| self.repo.read_file(path))?;
        let schemas = self.index.load_all_typedefs(&self.repo);
        with_savepoint(self.index.sql_conn(), "cascade_delete", || {
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
            for (id, _) in &plan {
                let doogat_type: Option<String> = self.index.sql_conn().query_row(
                    "SELECT type FROM doogats WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )?;
                let table = doogat_type
                    .as_deref()
                    .filter(|t| !t.is_empty() && *t != "_typedef");
                if let Some(table) = table {
                    self.index.cascade_junction_cleanup(&self.repo, table, id)?;
                }
                self.index.remove_doogat(id)?;
                if let Some(table) = table {
                    // Imported/unregistered types may have no materialized
                    // table. Its absence is expected; an actual DELETE error
                    // on an existing table must roll the operation back.
                    let exists: bool = self.index.sql_conn().query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                        params![table], |row| row.get(0),
                    )?;
                    if exists {
                        self.index.sql_conn().execute(
                            &format!("DELETE FROM \"{}\" WHERE id = ?1", escape_sql_ident(table)),
                            params![id],
                        )?;
                    }
                }
            }
            let writes: Vec<(&str, &str)> = edits
                .iter()
                .map(|e| (e.parsed.path.as_str(), e.content.as_str()))
                .collect();
            let deletes: Vec<&str> = plan.iter().map(|(_, path)| path.as_str()).collect();
            self.repo.commit_batch(&writes, &deletes, message)?;
            self.index.store_head(&self.repo.head_oid()?.0)?;
            Ok(())
        })?;
        // The mirror is derived; update it only after the authoritative write
        // succeeds, so a refused delete cannot remove live mirror records.
        for (id, _) in &plan {
            let _ = self.nosql.mirror_remove_doogat(id);
        }
        Ok(broken)
    }
}
