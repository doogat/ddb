use rusqlite::params;

use crate::error::Result;
use crate::types::{
    BrokenSequence, LinkDensityEntry, OrphanDoogat, RecentDoogat, SequenceInfo, SequenceNode,
    Suggestion, UnlinkedMention,
};

use super::Index;

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

/// Resolve the most relevant date for a doogat using the priority chain:
/// git revision date, then frontmatter date, then indexer updated_at.
fn resolve_last_date(
    repo: &(impl crate::traits::DoogatSource + crate::traits::GitHistory),
    path: &str,
    fm_date: Option<&str>,
    updated_at: Option<&str>,
) -> Option<(String, crate::types::DateSource)> {
    use crate::types::DateSource;

    if let Ok(Some(git_date)) = repo.revision_date(path) {
        Some((git_date, DateSource::GitRevision))
    } else if let Some(d) = fm_date {
        Some((d.to_string(), DateSource::FrontmatterDate))
    } else {
        updated_at.map(|u| (u.to_string(), DateSource::IndexerUpdatedAt))
    }
}

/// Parse a date, compute days since, and build a `StaleDoogat` if past threshold.
fn compute_staleness(
    id: &str,
    title: String,
    doogat_type: String,
    last_date: &str,
    source: crate::types::DateSource,
    threshold: u32,
    today: chrono::NaiveDate,
) -> Option<crate::types::StaleDoogat> {
    let naive = parse_date_to_naive(last_date)?;
    let days_since = (today - naive).num_days();
    if days_since < 0 {
        return None;
    }
    let days_since = days_since as u32;
    if days_since <= threshold {
        return None;
    }
    Some(crate::types::StaleDoogat {
        id: id.to_string(),
        title,
        doogat_type,
        last_updated: last_date.to_string(),
        date_source: source,
        days_stale: days_since - threshold,
        threshold_days: threshold,
    })
}

fn count_links(
    id: &str,
    title: &str,
    doogat_type: &str,
    path: &str,
    out_stmt: &mut rusqlite::Statement<'_>,
    in_stmt: &mut rusqlite::Statement<'_>,
) -> LinkDensityEntry {
    let outbound: usize = out_stmt
        .query_row(params![id], |row| row.get::<_, i64>(0))
        .unwrap_or(0) as usize;

    let inbound: usize = in_stmt
        .query_row(params![id, path], |row| row.get::<_, i64>(0))
        .unwrap_or(0) as usize;

    LinkDensityEntry {
        id: id.to_string(),
        title: title.to_string(),
        doogat_type: doogat_type.to_string(),
        inbound_links: inbound,
        outbound_links: outbound,
        density_score: inbound + outbound,
    }
}

fn normalize_suggestion_scores(mut results: Vec<Suggestion>) -> Vec<Suggestion> {
    let max = results.iter().map(|s| s.score).fold(0.0_f64, f64::max);
    if max > 0.0 {
        for s in &mut results {
            s.score /= max;
        }
    }
    results
}
