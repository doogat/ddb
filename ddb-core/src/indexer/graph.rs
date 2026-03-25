use rusqlite::params;

use crate::error::Result;
use crate::types::{
    BrokenSequence, OrphanDoogat, SequenceInfo, SequenceNode, Suggestion, UnlinkedMention,
};

use super::Index;

impl Index {
    /// Find all doogats linking to a given target.
    pub fn backlinks(&self, target_path: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT source_id FROM _ddb_links WHERE target_path = ?1")?;
        let ids = stmt.query_map(params![target_path], |row| row.get(0))?;
        let mut out = Vec::new();
        for id in ids {
            out.push(id?);
        }
        Ok(out)
    }

    /// Find all doogats linking to a target, returning (source_id, source_path).
    pub fn backlinking_doogat_paths(&self, target: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT l.source_id, z.path \
             FROM _ddb_links l JOIN doogats z ON l.source_id = z.id \
             WHERE l.target_path = ?1",
        )?;
        let rows = stmt.query_map(params![target], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Return (id, title) pairs of doogats with `resurrected: true` frontmatter.
    pub fn resurrected_doogats(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT z.id, z.title FROM doogats z \
             JOIN _ddb_fields f ON f.doogat_id = z.id \
             WHERE f.key = 'resurrected' AND f.value = 'true' \
             AND f.zone = 'Frontmatter'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Return (source_id, target_path) pairs where a link target has no matching doogat.
    pub fn broken_backlinks(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT l.source_id, l.target_path \
             FROM _ddb_links l \
             LEFT JOIN doogats z ON l.target_path = z.id \
             WHERE z.id IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ── Discovery queries ────────────────────────────────────────────

    /// Find doogats whose body mentions the target doogat's title without linking to it.
    pub fn unlinked_mentions(&self, target_id: &str) -> Result<Vec<UnlinkedMention>> {
        // Look up target doogat's title
        let title: String = match self.conn.query_row(
            "SELECT title FROM doogats WHERE id = ?1",
            params![target_id],
            |row| row.get(0),
        ) {
            Ok(t) => t,
            Err(_) => return Ok(vec![]),
        };

        if title.is_empty() {
            return Ok(vec![]);
        }

        // Build FTS5 phrase query — quote the title for phrase matching
        let phrase = format!("\"{}\"", title.replace('"', "\"\""));

        // Find all doogat IDs that link to the target (by path, ID, or alias)
        let target_path = self.resolve_path(target_id).unwrap_or_default();
        let target_id_str = target_id.to_string();

        let sql = "\
            SELECT z.id, z.title, snippet(_ddb_fts, 1, '<b>', '</b>', '...', 16) \
            FROM _ddb_fts \
            JOIN doogats z ON z.rowid = _ddb_fts.rowid \
            WHERE _ddb_fts MATCH ?1 \
              AND z.id != ?2 \
              AND z.id NOT IN ( \
                SELECT source_id FROM _ddb_links \
                WHERE target_path = ?3 OR target_path = ?4 \
                   OR target_path IN (SELECT alias FROM _ddb_aliases WHERE doogat_id = ?2) \
              ) \
            ORDER BY z.id";

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(
            params![phrase, target_id_str, target_path, target_id_str],
            |row| {
                Ok(UnlinkedMention {
                    source_id: row.get(0)?,
                    source_title: row.get(1)?,
                    snippet: row.get(2)?,
                })
            },
        )?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Suggest related doogats based on tag overlap and content similarity.
    pub fn suggest_links(
        &self,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<Suggestion>> {
        use std::collections::{HashMap, HashSet};

        // Get source doogat's tags
        let mut tag_stmt = self
            .conn
            .prepare("SELECT tag FROM _ddb_tags WHERE doogat_id = ?1")?;
        let source_tags: HashSet<String> = tag_stmt
            .query_map(params![source_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        if source_tags.is_empty() {
            // Fall back to content-only similarity
            return self.suggest_by_content(source_id, limit);
        }

        // Get source title for content similarity
        let source_title: String = self
            .conn
            .query_row(
                "SELECT title FROM doogats WHERE id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .unwrap_or_default();

        // Find candidates with at least one shared tag
        let mut candidate_tags: HashMap<String, HashSet<String>> = HashMap::new();
        let mut shared_stmt = self.conn.prepare(
            "SELECT DISTINCT t2.doogat_id, t2.tag \
             FROM _ddb_tags t1 \
             JOIN _ddb_tags t2 ON t1.tag = t2.tag \
             WHERE t1.doogat_id = ?1 AND t2.doogat_id != ?1",
        )?;
        let rows = shared_stmt.query_map(params![source_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for r in rows {
            let (id, tag) = r?;
            candidate_tags.entry(id).or_default().insert(tag);
        }

        // Get all tags for each candidate to compute Jaccard
        let mut all_tags_stmt = self
            .conn
            .prepare("SELECT tag FROM _ddb_tags WHERE doogat_id = ?1")?;

        // Collect already-linked IDs
        let linked_ids: HashSet<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT target_path FROM _ddb_links WHERE source_id = ?1")?;
            let rows = stmt.query_map(params![source_id], |row| row.get(0))?;
            let mut set = HashSet::new();
            for id in rows.flatten() {
                set.insert(id);
            }
            set
        };

        // Prepare alias lookup for linked-check
        let mut alias_stmt = self
            .conn
            .prepare("SELECT alias FROM _ddb_aliases WHERE doogat_id = ?1")?;

        let mut scored: Vec<(String, f64, Vec<String>)> = Vec::new();
        for (candidate_id, shared) in &candidate_tags {
            // Skip already-linked (by ID, path, or alias)
            if linked_ids.contains(candidate_id)
                || linked_ids.contains(&self.resolve_path(candidate_id).unwrap_or_default())
                || alias_stmt
                    .query_map(params![candidate_id], |row| row.get::<_, String>(0))
                    .ok()
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|alias| linked_ids.contains(&alias))
            {
                continue;
            }

            let all_candidate_tags: HashSet<String> = all_tags_stmt
                .query_map(params![candidate_id], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            let union_size = source_tags.union(&all_candidate_tags).count();
            let jaccard = if union_size > 0 {
                shared.len() as f64 / union_size as f64
            } else {
                0.0
            };

            let mut shared_list: Vec<String> = shared.iter().cloned().collect();
            shared_list.sort();

            scored.push((candidate_id.clone(), jaccard * 0.6, shared_list));
        }

        // Add content similarity via FTS5 BM25 if we have a title
        if !source_title.is_empty() {
            let phrase = format!("\"{}\"", source_title.replace('"', "\"\""));
            let mut fts_stmt = self.conn.prepare(
                "SELECT z.id, rank FROM _ddb_fts \
                 JOIN doogats z ON z.rowid = _ddb_fts.rowid \
                 WHERE _ddb_fts MATCH ?1 AND z.id != ?2 \
                 ORDER BY rank LIMIT 50",
            )?;
            let fts_rows = fts_stmt.query_map(params![phrase, source_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?;

            // BM25 rank is negative (lower = better). Normalize to 0..1
            let mut content_scores: Vec<(String, f64)> = Vec::new();
            for r in fts_rows {
                let (id, rank) = r?;
                content_scores.push((id, -rank)); // flip sign so higher = better
            }
            let max_score = content_scores
                .iter()
                .map(|(_, s)| *s)
                .fold(0.0_f64, f64::max);

            if max_score > 0.0 {
                let content_map: HashMap<String, f64> = content_scores
                    .into_iter()
                    .map(|(id, s)| (id, s / max_score))
                    .collect();

                // Merge content scores into existing candidates
                for (id, score, _) in &mut scored {
                    if let Some(&content_score) = content_map.get(id) {
                        *score += content_score * 0.4;
                    }
                }

                // Add content-only candidates not already in the list
                for (id, norm_score) in &content_map {
                    if !candidate_tags.contains_key(id)
                        && id != source_id
                        && !linked_ids.contains(id)
                        && !linked_ids.contains(&self.resolve_path(id).unwrap_or_default())
                        && !alias_stmt
                            .query_map(params![id.as_str()], |row| row.get::<_, String>(0))
                            .ok()
                            .into_iter()
                            .flatten()
                            .flatten()
                            .any(|alias| linked_ids.contains(&alias))
                    {
                        scored.push((id.clone(), norm_score * 0.4, vec![]));
                    }
                }
            }
        }

        // Sort by score descending, take top N
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        // Look up titles
        let mut title_stmt = self
            .conn
            .prepare("SELECT title FROM doogats WHERE id = ?1")?;
        let results: Vec<Suggestion> = scored
            .into_iter()
            .map(|(id, score, shared_tags)| {
                let title: String = title_stmt
                    .query_row(params![id], |row| row.get(0))
                    .unwrap_or_default();
                Suggestion {
                    id,
                    title,
                    score,
                    shared_tags,
                }
            })
            .collect();

        Ok(results)
    }

    /// Content-only suggestion fallback when source has no tags.
    fn suggest_by_content(
        &self,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<Suggestion>> {
        use std::collections::HashSet;

        let source_title: String = self
            .conn
            .query_row(
                "SELECT title FROM doogats WHERE id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .unwrap_or_default();

        if source_title.is_empty() {
            return Ok(vec![]);
        }

        let linked_ids: HashSet<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT target_path FROM _ddb_links WHERE source_id = ?1")?;
            let rows = stmt.query_map(params![source_id], |row| row.get(0))?;
            let mut set = HashSet::new();
            for id in rows.flatten() {
                set.insert(id);
            }
            set
        };

        let mut alias_stmt = self
            .conn
            .prepare("SELECT alias FROM _ddb_aliases WHERE doogat_id = ?1")?;

        let phrase = format!("\"{}\"", source_title.replace('"', "\"\""));
        let mut stmt = self.conn.prepare(
            "SELECT z.id, z.title, rank FROM _ddb_fts \
             JOIN doogats z ON z.rowid = _ddb_fts.rowid \
             WHERE _ddb_fts MATCH ?1 AND z.id != ?2 \
             ORDER BY rank LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![phrase, source_id, limit as i64 + 10], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;

        let mut results = Vec::new();
        for r in rows {
            let (id, title, rank) = r?;
            if linked_ids.contains(&id)
                || linked_ids.contains(&self.resolve_path(&id).unwrap_or_default())
                || alias_stmt
                    .query_map(params![&id], |row| row.get::<_, String>(0))
                    .ok()
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|alias| linked_ids.contains(&alias))
            {
                continue;
            }
            results.push(Suggestion {
                id,
                title,
                score: -rank, // flip sign
                shared_tags: vec![],
            });
            if results.len() >= limit {
                break;
            }
        }

        // Normalize scores
        let max = results.iter().map(|s| s.score).fold(0.0_f64, f64::max);
        if max > 0.0 {
            for s in &mut results {
                s.score /= max;
            }
        }

        Ok(results)
    }

    /// Find doogats past their type's staleness threshold.
    pub fn stale_doogats(
        &self,
        repo: &crate::git_ops::GitRepo,
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::StaleDoogat>> {
        use crate::types::{DateSource, StaleDoogat};

        // Load typedef thresholds
        let mut threshold_stmt = self
            .conn
            .prepare("SELECT z.title, z.path FROM doogats z WHERE z.type = '_typedef'")?;
        let typedef_rows = threshold_stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut thresholds: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        for r in typedef_rows {
            let (type_name, path) = r?;
            // Read the typedef to get stale_after_days
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

        if thresholds.is_empty() {
            return Ok(vec![]);
        }

        // Query candidate doogats
        let (sql, filter_val) = if let Some(t) = type_filter {
            if !thresholds.contains_key(t) {
                return Ok(vec![]);
            }
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

        type Row = (
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
        );
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Row> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        };

        let collected: Vec<Row> = if let Some(ref t) = filter_val {
            let rows = stmt.query_map(params![t], map_row)?;
            rows.filter_map(|r| r.ok()).collect()
        } else {
            let rows = stmt.query_map([], map_row)?;
            rows.filter_map(|r| r.ok()).collect()
        };

        let today = chrono::Utc::now().date_naive();
        let mut stale = Vec::new();

        for (id, title, doogat_type, fm_date, path, updated_at) in collected {
            let threshold = match thresholds.get(&doogat_type) {
                Some(&t) => t,
                None => continue,
            };

            // Date priority chain: git revision → frontmatter date → updated_at
            let (last_date, source) = if let Ok(Some(git_date)) = repo.revision_date(&path) {
                (git_date, DateSource::GitRevision)
            } else if let Some(ref d) = fm_date {
                (d.clone(), DateSource::FrontmatterDate)
            } else if let Some(ref u) = updated_at {
                (u.clone(), DateSource::IndexerUpdatedAt)
            } else {
                continue;
            };

            // Parse date and compute days since
            let parsed_date = parse_date_to_naive(&last_date);
            let Some(naive) = parsed_date else { continue };
            let days_since = (today - naive).num_days();
            if days_since < 0 {
                continue;
            }
            let days_since = days_since as u32;

            if days_since > threshold {
                stale.push(StaleDoogat {
                    id,
                    title,
                    doogat_type,
                    last_updated: last_date,
                    date_source: source,
                    days_stale: days_since - threshold,
                    threshold_days: threshold,
                });
            }
        }

        stale.sort_by(|a, b| b.days_stale.cmp(&a.days_stale));
        Ok(stale)
    }

    /// Find doogats with zero incoming backlinks.
    pub fn orphan_doogats(
        &self,
        type_filter: Option<&str>,
    ) -> Result<Vec<OrphanDoogat>> {
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
    pub fn sequence_tree(
        &self,
        id: &str,
        max_depth: usize,
    ) -> Result<Vec<(SequenceNode, usize)>> {
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

/// Parse a date string (ISO 8601 or YYYY-MM-DD) into a NaiveDate.
fn parse_date_to_naive(s: &str) -> Option<chrono::NaiveDate> {
    // Try ISO 8601 datetime first (e.g. 2026-03-16T20:51:04+00:00)
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.date_naive());
    }
    // Try date-only (e.g. 2026-03-16)
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    // Try ISO 8601 with non-standard offset format (e.g. 2026-03-16T20:51:04+0000)
    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z") {
        return Some(dt.date_naive());
    }
    None
}
