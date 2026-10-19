use rusqlite::params;

use crate::error::Result;
use crate::types::{
    BrokenSequence, LinkDensityEntry, OrphanDoogat, RecentDoogat, SequenceInfo, SequenceNode,
};

use super::staleness::{compute_staleness, count_links, parse_date_to_naive, resolve_last_date};
use crate::indexer::Index;

/// (id, title, type, date, path, updated_at)
type StalenessRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
);

impl Index {
    /// Load `stale_after_days` thresholds from all `_typedef` doogats.
    fn load_staleness_thresholds(
        &self,
        repo: &(impl crate::traits::DoogatSource + crate::traits::GitHistory),
    ) -> Result<std::collections::HashMap<String, u32>> {
        let mut stmt = self
            .conn
            .prepare("SELECT z.title, z.path FROM doogats z WHERE z.type = '_typedef'")?;
        let typedef_rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut thresholds = std::collections::HashMap::new();
        for r in typedef_rows {
            let (type_name, path) = r?;
            if let Ok(content) = repo.read_file(&path) {
                if let Ok(parsed) = crate::parser::parse(&content, &path) {
                    if let Some(days) = parsed
                        .meta
                        .extra
                        .get("stale_after_days")
                        .and_then(|v| v.as_f64())
                    {
                        thresholds.insert(type_name, days as u32);
                    }
                }
            }
        }
        Ok(thresholds)
    }

    /// Find doogats past their type's staleness threshold.
    pub fn stale_doogats(
        &self,
        repo: &(impl crate::traits::DoogatSource + crate::traits::GitHistory),
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::StaleDoogat>> {
        let thresholds = self.load_staleness_thresholds(repo)?;
        if thresholds.is_empty() {
            return Ok(vec![]);
        }

        if let Some(t) = type_filter {
            if !thresholds.contains_key(t) {
                return Ok(vec![]);
            }
        }

        let candidates = self.query_stale_candidates(type_filter)?;
        let today = chrono::Utc::now().date_naive();
        let mut stale = Vec::new();

        for (id, title, doogat_type, fm_date, path, updated_at) in candidates {
            let threshold = match thresholds.get(&doogat_type) {
                Some(&t) => t,
                None => continue,
            };

            let Some((last_date, source)) =
                resolve_last_date(repo, &path, fm_date.as_deref(), updated_at.as_deref())
            else {
                continue;
            };

            if let Some(entry) = compute_staleness(
                &id,
                title,
                doogat_type,
                &last_date,
                source,
                threshold,
                today,
            ) {
                stale.push(entry);
            }
        }

        stale.sort_by_key(|s| std::cmp::Reverse(s.days_stale));
        Ok(stale)
    }

    fn query_stale_candidates(&self, type_filter: Option<&str>) -> Result<Vec<StalenessRow>> {
        let (sql, filter_val) = if let Some(t) = type_filter {
            (
                "SELECT id, title, type, date, path, updated_at FROM doogats \
                 WHERE type = ?1 AND path NOT LIKE 'ddb/_typedef/%'"
                    .to_string(),
                Some(t.to_string()),
            )
        } else {
            (
                "SELECT id, title, type, date, path, updated_at FROM doogats \
                 WHERE path NOT LIKE 'ddb/_typedef/%'"
                    .to_string(),
                None,
            )
        };

        let mut stmt = self.conn.prepare(&sql)?;

        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<StalenessRow> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        };

        let collected: Vec<StalenessRow> = if let Some(ref t) = filter_val {
            stmt.query_map(params![t], map_row)?
                .filter_map(|r| r.ok())
                .collect()
        } else {
            stmt.query_map([], map_row)?
                .filter_map(|r| r.ok())
                .collect()
        };

        Ok(collected)
    }

    /// Find doogats with zero incoming backlinks.
    pub fn orphan_doogats(&self, type_filter: Option<&str>) -> Result<Vec<OrphanDoogat>> {
        let base = "\
            SELECT z.id, z.title, z.type, \
                   (SELECT COUNT(*) FROM _ddb_links WHERE source_id = z.id) AS outgoing \
            FROM doogats z \
            WHERE z.path NOT LIKE 'ddb/_typedef/%' \
              AND NOT EXISTS ( \
                SELECT 1 FROM _ddb_links l \
                WHERE l.target_path = z.path \
                   OR l.target_path = z.id \
                   OR l.target_path IN (SELECT alias FROM _ddb_aliases WHERE doogat_id = z.id) \
              )";

        let sql = if type_filter.is_some() {
            format!("{base} AND z.type = ?1 ORDER BY z.id")
        } else {
            format!("{base} ORDER BY z.id")
        };

        let mut stmt = self.conn.prepare(&sql)?;

        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<OrphanDoogat> {
            Ok(OrphanDoogat {
                id: row.get(0)?,
                title: row.get(1)?,
                doogat_type: row.get(2)?,
                outgoing_links: row.get::<_, i64>(3)? as usize,
            })
        };

        let out: Vec<OrphanDoogat> = if let Some(t) = type_filter {
            let rows = stmt.query_map(params![t], map_row)?;
            rows.filter_map(|r| r.ok()).collect()
        } else {
            let rows = stmt.query_map([], map_row)?;
            rows.filter_map(|r| r.ok()).collect()
        };

        Ok(out)
    }

    /// Build the SQL query and optional type filter value for `recent_doogats`.
    fn build_recent_query(type_filter: Option<&str>) -> (String, Option<String>) {
        if let Some(t) = type_filter {
            (
                "SELECT id, title, type, COALESCE(NULLIF(date,''), updated_at) AS effective \
                 FROM doogats \
                 WHERE path NOT LIKE 'ddb/_typedef/%' AND type = ?1 \
                   AND COALESCE(NULLIF(date,''), updated_at) >= ?2 \
                 ORDER BY effective DESC"
                    .to_string(),
                Some(t.to_string()),
            )
        } else {
            (
                "SELECT id, title, type, COALESCE(NULLIF(date,''), updated_at) AS effective \
                 FROM doogats \
                 WHERE path NOT LIKE 'ddb/_typedef/%' \
                   AND COALESCE(NULLIF(date,''), updated_at) >= ?1 \
                 ORDER BY effective DESC"
                    .to_string(),
                None,
            )
        }
    }

    /// Find doogats modified within a recent time window.
    pub fn recent_doogats(
        &self,
        days: u32,
        type_filter: Option<&str>,
    ) -> Result<Vec<RecentDoogat>> {
        let today = chrono::Utc::now().date_naive();
        let cutoff = today - chrono::Duration::days(i64::from(days));
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();

        let (sql, filter_val) = Self::build_recent_query(type_filter);
        let mut stmt = self.conn.prepare(&sql)?;

        type Row = (String, String, String, String);
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Row> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        };

        let collected: Vec<Row> = if let Some(ref t) = filter_val {
            let rows = stmt.query_map(params![t, cutoff_str], map_row)?;
            rows.filter_map(|r| r.ok()).collect()
        } else {
            let rows = stmt.query_map(params![cutoff_str], map_row)?;
            rows.filter_map(|r| r.ok()).collect()
        };

        let recent: Vec<RecentDoogat> = collected
            .into_iter()
            .filter_map(|(id, title, doogat_type, effective_date)| {
                parse_date_to_naive(&effective_date)?;
                Some(RecentDoogat {
                    id,
                    title,
                    doogat_type,
                    last_modified: effective_date,
                })
            })
            .collect();

        Ok(recent)
    }

    /// Report inbound/outbound link counts per doogat.
    pub fn link_density(&self, type_filter: Option<&str>) -> Result<Vec<LinkDensityEntry>> {
        let base_sql = if type_filter.is_some() {
            "SELECT z.id, z.title, z.type, z.path FROM doogats z \
             WHERE z.path NOT LIKE 'ddb/_typedef/%' AND z.type = ?1"
        } else {
            "SELECT z.id, z.title, z.type, z.path FROM doogats z \
             WHERE z.path NOT LIKE 'ddb/_typedef/%'"
        };

        let mut stmt = self.conn.prepare(base_sql)?;

        type Row = (String, String, String, String);
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Row> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        };

        let doogats: Vec<Row> = if let Some(t) = type_filter {
            stmt.query_map(params![t], map_row)?
                .filter_map(|r| r.ok())
                .collect()
        } else {
            stmt.query_map([], map_row)?
                .filter_map(|r| r.ok())
                .collect()
        };

        let mut out_stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM _ddb_links WHERE source_id = ?1")?;
        let mut in_stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM _ddb_links \
             WHERE target_path = ?1 OR target_path = ?2 \
                OR target_path IN (SELECT alias FROM _ddb_aliases WHERE doogat_id = ?1)",
        )?;

        let mut entries = Vec::with_capacity(doogats.len());
        for (id, title, doogat_type, path) in &doogats {
            entries.push(count_links(
                id,
                title,
                doogat_type,
                path,
                &mut out_stmt,
                &mut in_stmt,
            ));
        }

        entries.sort_by_key(|e| std::cmp::Reverse(e.density_score));
        Ok(entries)
    }

    // ── Sequence queries ──────────────────────────────────────────

    /// Return direct children of a doogat in a sequence (sorted by ID).
    pub fn sequence_children(&self, id: &str) -> Result<Vec<SequenceNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT z.id, z.title FROM _ddb_fields f \
             JOIN doogats z ON z.id = f.doogat_id \
             WHERE f.key = 'sequence' AND f.value = ?1 \
             ORDER BY z.id",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(SequenceNode {
                id: row.get(0)?,
                title: row.get(1)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Walk up the parent chain from a doogat to the sequence root.
    ///
    /// Returns the path from root to self (inclusive). Breaks after 100
    /// iterations to guard against cycles.
    pub fn sequence_breadcrumb(&self, id: &str) -> Result<Vec<SequenceNode>> {
        let mut chain = Vec::new();
        let mut current = id.to_string();
        let mut seen = std::collections::HashSet::new();

        for _ in 0..100 {
            if !seen.insert(current.clone()) {
                break; // cycle detected
            }
            // Check doogat exists; stop if it doesn't (broken mid-chain ref)
            let title: Option<String> = self
                .conn
                .query_row(
                    "SELECT title FROM doogats WHERE id = ?1",
                    params![&current],
                    |row| row.get(0),
                )
                .ok();
            let Some(title) = title else { break };
            chain.push(SequenceNode {
                id: current.clone(),
                title,
            });

            let parent: Option<String> = self
                .conn
                .query_row(
                    "SELECT f.value FROM _ddb_fields f \
                     WHERE f.doogat_id = ?1 AND f.key = 'sequence'",
                    params![&current],
                    |row| row.get(0),
                )
                .ok();
            match parent {
                Some(pid) => current = pid,
                None => break,
            }
        }

        chain.reverse();
        Ok(chain)
    }

    /// Full sequence context for a doogat: parent, children, and breadcrumb.
    pub fn sequence_info(&self, id: &str) -> Result<SequenceInfo> {
        let parent: Option<SequenceNode> = self
            .conn
            .query_row(
                "SELECT f.value FROM _ddb_fields f \
                 WHERE f.doogat_id = ?1 AND f.key = 'sequence'",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .map(|pid| {
                let title: String = self
                    .conn
                    .query_row(
                        "SELECT title FROM doogats WHERE id = ?1",
                        params![&pid],
                        |row| row.get(0),
                    )
                    .unwrap_or_default();
                SequenceNode { id: pid, title }
            });

        let children = self.sequence_children(id)?;
        let breadcrumb = self.sequence_breadcrumb(id)?;

        Ok(SequenceInfo {
            parent,
            children,
            breadcrumb,
        })
    }

    /// Recursive subtree rooted at `id`. Returns nodes with their depth (0 = root).
    ///
    /// Depth-limited to `max_depth` to guard against cycles or very deep trees.
    pub fn sequence_tree(&self, id: &str, max_depth: usize) -> Result<Vec<(SequenceNode, usize)>> {
        let mut result = Vec::new();
        self.sequence_tree_inner(id, 0, max_depth, &mut result)?;
        Ok(result)
    }

    fn sequence_tree_inner(
        &self,
        id: &str,
        depth: usize,
        max_depth: usize,
        out: &mut Vec<(SequenceNode, usize)>,
    ) -> Result<()> {
        let title: String = self
            .conn
            .query_row(
                "SELECT title FROM doogats WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap_or_default();
        out.push((
            SequenceNode {
                id: id.to_string(),
                title,
            },
            depth,
        ));
        if depth >= max_depth {
            return Ok(());
        }
        let children = self.sequence_children(id)?;
        for child in &children {
            self.sequence_tree_inner(&child.id, depth + 1, max_depth, out)?;
        }
        Ok(())
    }

    /// Find doogats whose `sequence` field references a non-existent parent.
    pub fn broken_sequences(&self) -> Result<Vec<BrokenSequence>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.doogat_id, f.value FROM _ddb_fields f \
             LEFT JOIN doogats z ON z.id = f.value \
             WHERE f.key = 'sequence' AND z.id IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BrokenSequence {
                doogat_id: row.get(0)?,
                broken_parent_id: row.get(1)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
