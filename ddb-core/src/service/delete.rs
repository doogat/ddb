use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::parser;

use crate::traits::{GitBackend, IndexPort};

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Delete a doogat by ID. Returns broken backlinks `(source_id, source_path)`.
    ///
    /// Cascade behavior:
    /// - Junction table rows and dangling wikilinks in referencing files
    ///   are cleaned up atomically in a single git commit.
    /// - PRD 00129 §2: typed-table rows that reference the deleted id
    ///   through an `ON DELETE CASCADE` column are deleted recursively in
    ///   the same commit. Cycle detection rejects with `CASCADE_CYCLE`.
    pub fn delete_doogat(&self, id: &str, message: &str) -> Result<Vec<(String, String)>> {
        self.ensure_fresh()?;
        // Build the full cascade plan up front so the commit covers the
        // whole graph atomically (parent + every cascade-collected
        // descendant + their reference edits).
        let plan = self.build_cascade_delete_plan(id)?;
        self.execute_delete_plan(plan, id, message)
    }

    /// PRD 00129 §2: walk the CASCADE graph rooted at `id`, returning the
    /// ordered list of (id, path) pairs to delete. Cycle detection rejects
    /// with `CASCADE_CYCLE` listing the offending tables.
    fn build_cascade_delete_plan(&self, id: &str) -> Result<Vec<(String, String)>> {
        use std::collections::BTreeSet;
        let root_path = self.index.resolve_path(id)?;
        let mut ordered: Vec<(String, String)> = vec![(id.to_string(), root_path)];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        seen.insert(id.to_string());
        // Process FIFO so children of children land in a stable, depth-first
        // order. Cycle = revisiting a parent we've already enqueued; we
        // collect the tables involved for the error context.
        let mut cursor = 0;
        while cursor < ordered.len() {
            let parent = ordered[cursor].0.clone();
            cursor += 1;
            // RESTRICT check applies at every level: if any cascade-deleted
            // child has a RESTRICT-marked back-reference, the whole delete
            // rejects.
            self.index
                .check_restrict_blocks_delete(&self.repo, &parent)?;
            let children = self.index.collect_cascade_children(&self.repo, &parent)?;
            for (child_table, child_id) in children {
                if !seen.insert(child_id.clone()) {
                    return Err(DoogatError::cascade_cycle([child_table, parent.clone()]));
                }
                let child_path = match self.index.resolve_path(&child_id) {
                    Ok(p) => p,
                    Err(_) => continue, // child already gone? skip silently
                };
                ordered.push((child_id, child_path));
            }
        }
        Ok(ordered)
    }

    /// Execute a pre-collected cascade plan: collect ref edits, update the
    /// index, and commit every deletion + edit in a single batch.
    fn execute_delete_plan(
        &self,
        plan: Vec<(String, String)>,
        root_id: &str,
        message: &str,
    ) -> Result<Vec<(String, String)>> {
        use std::collections::BTreeSet;
        let broken = self.index.backlinking_doogat_paths(root_id)?;
        // Paths that will be deleted in this batch — we must not emit a
        // write edit for them (commit_batch can't both write and delete
        // the same path in one commit; git2 errors on the conflicting
        // index op). Edits to other backlinking files are still emitted.
        let delete_paths: BTreeSet<&str> = plan.iter().map(|(_, p)| p.as_str()).collect();
        let mut ref_edits: Vec<(String, String)> = Vec::new();
        for (id, path) in &plan {
            let edits = self.collect_ref_edits(id, path)?;
            for (p, c) in edits {
                if delete_paths.contains(p.as_str()) {
                    continue;
                }
                ref_edits.push((p, c));
            }
        }
        for (id, _path) in &plan {
            let doogat_type: Option<String> = self
                .index
                .sql_conn()
                .query_row(
                    "SELECT type FROM doogats WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .ok();
            self.index.remove_doogat(id)?;
            self.nosql_remove_doogat(id);
            if let Some(ref dtype) = doogat_type {
                if !dtype.is_empty() && dtype != "_typedef" {
                    let _ = self.index.sql_conn().execute(
                        &format!("DELETE FROM \"{}\" WHERE id = ?1", dtype),
                        params![id],
                    );
                    self.index.cascade_junction_cleanup(&self.repo, dtype, id)?;
                }
            }
        }
        let writes: Vec<(&str, &str)> = ref_edits
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        let deletes: Vec<&str> = plan.iter().map(|(_, p)| p.as_str()).collect();
        self.repo.commit_batch(&writes, &deletes, message)?;
        self.index.store_head(&self.repo.head_oid()?.0)?;
        Ok(broken)
    }

    /// Collect reference section edits needed when deleting a doogat.
    /// Returns `(path, new_content)` pairs; does NOT commit.
    fn collect_ref_edits(
        &self,
        deleted_id: &str,
        deleted_path: &str,
    ) -> Result<Vec<(String, String)>> {
        let sources = self.index.backlinks_by_target(deleted_id, deleted_path)?;
        let mut edits = Vec::new();
        for (source_id, source_path) in &sources {
            let content = self.repo.read_file(source_path)?;
            let mut parsed = parser::parse(&content, source_path)?;

            let old_section = parsed.reference_section.clone();
            let new_lines: Vec<&str> = old_section
                .lines()
                .filter(|line| {
                    !line.contains(&format!("[[{deleted_id}]]"))
                        && !line.contains(&format!("[[{deleted_path}]]"))
                })
                .collect();
            let new_section = if new_lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", new_lines.join("\n"))
            };

            if new_section == old_section {
                continue;
            }
            parsed.reference_section = new_section;
            let new_content = parser::serialize(&parsed);

            // Re-index and rematerialize
            let re_parsed = parser::parse(&new_content, source_path)?;
            self.index.index_doogat(&re_parsed)?;
            if let Some(ref stype) = re_parsed.meta.doogat_type {
                let schemas = self.index.load_all_typedefs(&self.repo);
                if let Some(schema) = schemas.get(stype.as_str()) {
                    self.index
                        .materialize_single(schema, source_id, &re_parsed)?;
                }
            }

            edits.push((source_path.to_string(), new_content));
        }
        Ok(edits)
    }

    /// Best-effort removal from the NoSQL mirror via the injected port.
    fn nosql_remove_doogat(&self, id: &str) {
        let _ = self.nosql.mirror_remove_doogat(id);
    }
}
