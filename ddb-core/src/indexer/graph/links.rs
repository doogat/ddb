use rusqlite::params;

use crate::error::Result;
use crate::types::{Suggestion, UnlinkedMention};

use super::staleness::normalize_suggestion_scores;
use crate::indexer::Index;

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

    /// Find all doogats linking to a target by both bare ID and file path.
    /// Returns deduplicated `(source_id, source_path)` pairs.
    pub fn backlinks_by_target(
        &self,
        target_id: &str,
        target_path: &str,
    ) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        for target in &[target_id, target_path] {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT l.source_id, z.path \
                 FROM _ddb_links l JOIN doogats z ON l.source_id = z.id \
                 WHERE l.target_path = ?1",
            )?;
            let rows = stmt.query_map(params![target], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, path) = row?;
                if !out.iter().any(|(sid, _): &(String, String)| sid == &id) {
                    out.push((id, path));
                }
            }
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
    pub fn suggest_links(&self, source_id: &str, limit: usize) -> Result<Vec<Suggestion>> {
        let source_tags = self.fetch_tags(source_id)?;
        if source_tags.is_empty() {
            return self.suggest_by_content(source_id, limit);
        }

        let source_title = self.fetch_title(source_id);
        let candidate_tags = self.find_tag_candidates(source_id)?;
        let linked_ids = self.collect_linked_ids(source_id)?;
        let mut alias_stmt = self
            .conn
            .prepare("SELECT alias FROM _ddb_aliases WHERE doogat_id = ?1")?;

        let mut scored =
            self.compute_tag_scores(&source_tags, &candidate_tags, &linked_ids, &mut alias_stmt)?;
        let content_map = self.query_content_scores(source_id, &source_title)?;
        self.merge_content_scores(
            source_id,
            &content_map,
            &candidate_tags,
            &linked_ids,
            &mut alias_stmt,
            &mut scored,
        )?;
        self.build_suggestions(&mut scored, limit)
    }

    fn fetch_tags(&self, doogat_id: &str) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tag FROM _ddb_tags WHERE doogat_id = ?1")?;
        let tags: std::collections::HashSet<String> = stmt
            .query_map(params![doogat_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(tags)
    }

    /// Fetch the title for a doogat, returning empty string if missing.
    fn fetch_title(&self, doogat_id: &str) -> String {
        self.conn
            .query_row(
                "SELECT title FROM doogats WHERE id = ?1",
                params![doogat_id],
                |row| row.get(0),
            )
            .unwrap_or_default()
    }

    /// Find candidates sharing at least one tag with `source_id`.
    fn find_tag_candidates(
        &self,
        source_id: &str,
    ) -> Result<std::collections::HashMap<String, std::collections::HashSet<String>>> {
        use std::collections::{HashMap, HashSet};

        let mut candidate_tags: HashMap<String, HashSet<String>> = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t2.doogat_id, t2.tag \
             FROM _ddb_tags t1 \
             JOIN _ddb_tags t2 ON t1.tag = t2.tag \
             WHERE t1.doogat_id = ?1 AND t2.doogat_id != ?1",
        )?;
        let rows = stmt.query_map(params![source_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for r in rows {
            let (id, tag) = r?;
            candidate_tags.entry(id).or_default().insert(tag);
        }
        Ok(candidate_tags)
    }

    /// Collect target IDs already linked from `source_id`.
    fn collect_linked_ids(&self, source_id: &str) -> Result<std::collections::HashSet<String>> {
        use std::collections::HashSet;

        let mut stmt = self
            .conn
            .prepare("SELECT target_path FROM _ddb_links WHERE source_id = ?1")?;
        let rows = stmt.query_map(params![source_id], |row| row.get(0))?;
        let mut set = HashSet::new();
        for id in rows.flatten() {
            set.insert(id);
        }
        Ok(set)
    }

    /// Check whether `candidate_id` is already linked (by ID, resolved path, or alias).
    fn is_candidate_linked(
        &self,
        candidate_id: &str,
        linked_ids: &std::collections::HashSet<String>,
        alias_stmt: &mut rusqlite::Statement<'_>,
    ) -> Result<bool> {
        if linked_ids.contains(candidate_id) {
            return Ok(true);
        }
        if linked_ids.contains(&self.resolve_path(candidate_id).unwrap_or_default()) {
            return Ok(true);
        }
        let has_alias_link = alias_stmt
            .query_map(params![candidate_id], |row| row.get::<_, String>(0))?
            .flatten()
            .any(|alias| linked_ids.contains(&alias));
        Ok(has_alias_link)
    }

    /// Score candidates by Jaccard tag similarity, skipping already-linked ones.
    fn compute_tag_scores(
        &self,
        source_tags: &std::collections::HashSet<String>,
        candidate_tags: &std::collections::HashMap<String, std::collections::HashSet<String>>,
        linked_ids: &std::collections::HashSet<String>,
        alias_stmt: &mut rusqlite::Statement<'_>,
    ) -> Result<Vec<(String, f64, Vec<String>)>> {
        let mut all_tags_stmt = self
            .conn
            .prepare("SELECT tag FROM _ddb_tags WHERE doogat_id = ?1")?;

        let mut scored: Vec<(String, f64, Vec<String>)> = Vec::new();
        for (candidate_id, shared) in candidate_tags {
            if self.is_candidate_linked(candidate_id, linked_ids, alias_stmt)? {
                continue;
            }

            let all_candidate_tags: std::collections::HashSet<String> = all_tags_stmt
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
        Ok(scored)
    }

    /// Query FTS5 BM25 content scores and normalize to 0..1.
    fn query_content_scores(
        &self,
        source_id: &str,
        source_title: &str,
    ) -> Result<std::collections::HashMap<String, f64>> {
        use std::collections::HashMap;

        if source_title.is_empty() {
            return Ok(HashMap::new());
        }

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

        let mut raw: Vec<(String, f64)> = Vec::new();
        for r in fts_rows {
            let (id, rank) = r?;
            raw.push((id, -rank));
        }
        let max_score = raw.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
        if max_score <= 0.0 {
            return Ok(HashMap::new());
        }

        Ok(raw.into_iter().map(|(id, s)| (id, s / max_score)).collect())
    }

    /// Merge content scores into existing tag-scored candidates and add content-only candidates.
    fn merge_content_scores(
        &self,
        source_id: &str,
        content_map: &std::collections::HashMap<String, f64>,
        candidate_tags: &std::collections::HashMap<String, std::collections::HashSet<String>>,
        linked_ids: &std::collections::HashSet<String>,
        alias_stmt: &mut rusqlite::Statement<'_>,
        scored: &mut Vec<(String, f64, Vec<String>)>,
    ) -> Result<()> {
        for (id, score, _) in scored.iter_mut() {
            if let Some(&content_score) = content_map.get(id) {
                *score += content_score * 0.4;
            }
        }

        for (id, norm_score) in content_map {
            if candidate_tags.contains_key(id) || id == source_id {
                continue;
            }
            if self.is_candidate_linked(id, linked_ids, alias_stmt)? {
                continue;
            }
            scored.push((id.clone(), norm_score * 0.4, vec![]));
        }

        Ok(())
    }

    /// Sort scored candidates, truncate to limit, and look up titles.
    fn build_suggestions(
        &self,
        scored: &mut Vec<(String, f64, Vec<String>)>,
        limit: usize,
    ) -> Result<Vec<Suggestion>> {
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        let mut title_stmt = self
            .conn
            .prepare("SELECT title FROM doogats WHERE id = ?1")?;
        let results = scored
            .iter()
            .map(|(id, score, shared_tags)| {
                let title: String = title_stmt
                    .query_row(params![id], |row| row.get(0))
                    .unwrap_or_default();
                Suggestion {
                    id: id.clone(),
                    title,
                    score: *score,
                    shared_tags: shared_tags.clone(),
                }
            })
            .collect();

        Ok(results)
    }

    /// Content-only suggestion fallback when source has no tags.
    fn suggest_by_content(&self, source_id: &str, limit: usize) -> Result<Vec<Suggestion>> {
        let source_title = self.fetch_title(source_id);
        if source_title.is_empty() {
            return Ok(vec![]);
        }

        let linked_ids = self.collect_linked_ids(source_id)?;
        let mut alias_stmt = self
            .conn
            .prepare("SELECT alias FROM _ddb_aliases WHERE doogat_id = ?1")?;

        let results = self.query_content_suggestions(
            source_id,
            &source_title,
            limit,
            &linked_ids,
            &mut alias_stmt,
        )?;
        Ok(normalize_suggestion_scores(results))
    }

    /// Query FTS5 for content-similar doogats, filtering already-linked ones.
    fn query_content_suggestions(
        &self,
        source_id: &str,
        source_title: &str,
        limit: usize,
        linked_ids: &std::collections::HashSet<String>,
        alias_stmt: &mut rusqlite::Statement<'_>,
    ) -> Result<Vec<Suggestion>> {
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
            if self.is_candidate_linked(&id, linked_ids, alias_stmt)? {
                continue;
            }
            results.push(Suggestion {
                id,
                title,
                score: -rank,
                shared_tags: vec![],
            });
            if results.len() >= limit {
                break;
            }
        }
        Ok(results)
    }
}
