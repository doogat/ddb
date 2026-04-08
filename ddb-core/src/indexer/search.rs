use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::types::{
    PaginatedSearchResult, SearchFilters, SearchResult, TagEntry, TagQueryFilter,
};

use super::filter::escape_sql_ident;
use super::Index;

impl Index {
    /// Full-text search with snippets and ranking.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.search_hits(query, None, &SearchFilters::default())
    }

    /// Paginated full-text search with snippets, ranking, and total count.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.search_paginated_filtered(query, limit, offset, &SearchFilters::default())
    }

    /// Paginated full-text search with optional type/tag/field filters.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search_paginated_filtered(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        filters: &SearchFilters,
    ) -> Result<PaginatedSearchResult> {
        let (filter_clauses, filter_params) = self.build_filter_clauses(filters);
        let boost_type = filters
            .types
            .as_ref()
            .and_then(|t| if t.len() == 1 { Some(t[0].as_str()) } else { None });
        let mut hits = self.search_hits_inner(query, Some((limit, offset)), &filter_clauses, filter_params.clone(), boost_type)?;
        self.enrich_search_hits(&mut hits);

        let negation = self.build_negation_plan(query);
        let filter_sql = filter_clauses.join(" ");

        let (count_sql, all_params) = match &negation {
            Some((Some(pos_query), neg_clauses, neg_params)) => {
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                    vec![Box::new(pos_query.clone())];
                for p in &filter_params {
                    params.push(Box::new(p.clone()));
                }
                let neg_sql = self.resolve_negation_clauses(neg_clauses, neg_params, &mut params);
                let sql = format!(
                    "SELECT COUNT(*) FROM _ddb_fts \
                     JOIN doogats z ON z.rowid = _ddb_fts.rowid \
                     WHERE _ddb_fts MATCH ?1 {filter_sql}{neg_sql}"
                );
                (sql, params)
            }
            Some((None, neg_clauses, neg_params)) => {
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
                let adjusted_filter_sql = Self::reindex_filter_sql(&filter_sql, &filter_params, &mut params);
                let neg_sql = self.resolve_negation_clauses(neg_clauses, neg_params, &mut params);
                let conditions = format!("{adjusted_filter_sql}{neg_sql}");
                let trimmed = conditions
                    .strip_prefix(" AND ")
                    .or_else(|| conditions.strip_prefix("AND "))
                    .unwrap_or(&conditions);
                let sql = if trimmed.is_empty() {
                    "SELECT COUNT(*) FROM doogats".to_string()
                } else {
                    format!("SELECT COUNT(*) FROM doogats z WHERE {trimmed}")
                };
                (sql, params)
            }
            None => {
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                    vec![Box::new(query.to_string())];
                for p in filter_params {
                    params.push(Box::new(p));
                }
                let sql = if filter_sql.is_empty() {
                    "SELECT COUNT(*) FROM _ddb_fts WHERE _ddb_fts MATCH ?1".to_string()
                } else {
                    format!(
                        "SELECT COUNT(*) FROM _ddb_fts \
                         JOIN doogats z ON z.rowid = _ddb_fts.rowid \
                         WHERE _ddb_fts MATCH ?1 {filter_sql}"
                    )
                };
                (sql, params)
            }
        };

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| &**p).collect();
        let total_count: usize = self.conn
            .query_row(&count_sql, param_refs.as_slice(), |row| row.get(0))
            .map_err(|e| Self::classify_search_error(e, query))?;

        Ok(PaginatedSearchResult { hits, total_count })
    }

    fn search_hits(
        &self,
        query: &str,
        pagination: Option<(usize, usize)>,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>> {
        let (filter_clauses, filter_params) = self.build_filter_clauses(filters);
        let boost_type = filters
            .types
            .as_ref()
            .and_then(|t| if t.len() == 1 { Some(t[0].as_str()) } else { None });
        let mut hits = self.search_hits_inner(query, pagination, &filter_clauses, filter_params, boost_type)?;
        self.enrich_search_hits(&mut hits);
        Ok(hits)
    }

    /// Look up the max search_boost for a type from the `_ddb_boost` table.
    fn lookup_boost(&self, type_name: &str) -> f64 {
        self.conn
            .query_row(
                "SELECT max_boost FROM _ddb_boost WHERE type_name = ?1",
                params![type_name],
                |row| row.get(0),
            )
            .unwrap_or(1.0)
    }

    fn search_hits_inner(
        &self,
        query: &str,
        pagination: Option<(usize, usize)>,
        filter_clauses: &[String],
        filter_params: Vec<String>,
        boost_type: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let negation = self.build_negation_plan(query);
        let filter_sql = filter_clauses.join(" ");

        let (base, mut all_params) = match &negation {
            Some((Some(pos_query), neg_clauses, neg_params)) => {
                // Has positive + negative terms
                let boost = boost_type.map_or(1.0, |t| self.lookup_boost(t));
                let order_clause = format!("ORDER BY bm25(_ddb_fts, 1.0, 1.0, 1.0, {boost})");
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                    vec![Box::new(pos_query.clone())];
                for p in filter_params {
                    params.push(Box::new(p));
                }
                let neg_sql = self.resolve_negation_clauses(neg_clauses, neg_params, &mut params);
                let sql = format!(
                    "SELECT z.id, z.title, z.path, \
                     snippet(_ddb_fts, 1, '<b>', '</b>', '...', 32), rank, z.updated_at, \
                     z.type, z.date \
                     FROM _ddb_fts \
                     JOIN doogats z ON z.rowid = _ddb_fts.rowid \
                     WHERE _ddb_fts MATCH ?1 {filter_sql}{neg_sql} \
                     {order_clause}"
                );
                (sql, params)
            }
            Some((None, neg_clauses, neg_params)) => {
                // All-negative query: scan doogats directly, no FTS MATCH
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
                // Filter params start at ?1 for all-negative (no FTS MATCH param)
                let adjusted_filter_sql = Self::reindex_filter_sql(&filter_sql, &filter_params, &mut params);
                let neg_sql = self.resolve_negation_clauses(neg_clauses, neg_params, &mut params);
                let where_clause = if adjusted_filter_sql.is_empty() && neg_sql.is_empty() {
                    String::new()
                } else {
                    let conditions = format!("{adjusted_filter_sql}{neg_sql}");
                    // Strip leading "AND " or " AND " to form valid WHERE
                    let trimmed = conditions
                        .strip_prefix(" AND ")
                        .or_else(|| conditions.strip_prefix("AND "))
                        .unwrap_or(&conditions);
                    format!("WHERE {trimmed}")
                };
                let sql = format!(
                    "SELECT z.id, z.title, z.path, \
                     '' AS snippet, 0.0 AS rank, z.updated_at, \
                     z.type, z.date \
                     FROM doogats z \
                     {where_clause} \
                     ORDER BY z.title"
                );
                (sql, params)
            }
            None => {
                // No negation - original behavior
                let boost = boost_type.map_or(1.0, |t| self.lookup_boost(t));
                let order_clause = format!("ORDER BY bm25(_ddb_fts, 1.0, 1.0, 1.0, {boost})");
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                    vec![Box::new(query.to_string())];
                for p in filter_params {
                    params.push(Box::new(p));
                }
                let sql = format!(
                    "SELECT z.id, z.title, z.path, \
                     snippet(_ddb_fts, 1, '<b>', '</b>', '...', 32), rank, z.updated_at, \
                     z.type, z.date \
                     FROM _ddb_fts \
                     JOIN doogats z ON z.rowid = _ddb_fts.rowid \
                     WHERE _ddb_fts MATCH ?1 {filter_sql} \
                     {order_clause}"
                );
                (sql, params)
            }
        };

        let sql = match pagination {
            Some(_) => {
                let limit_idx = all_params.len() + 1;
                let offset_idx = all_params.len() + 2;
                format!("{base} LIMIT ?{limit_idx} OFFSET ?{offset_idx}")
            }
            None => base,
        };

        if let Some((limit, offset)) = pagination {
            all_params.push(Box::new(limit as i64));
            all_params.push(Box::new(offset as i64));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| &**p).collect();
        let mut stmt = self.conn.prepare(&sql)
            .map_err(|e| Self::classify_search_error(e, query))?;
        let rows = stmt
            .query_map(param_refs.as_slice(), Self::map_search_row)
            .map_err(|e| Self::classify_search_error(e, query))?;

        let mut hits = Vec::new();
        for r in rows {
            hits.push(r.map_err(|e| Self::classify_search_error(e, query))?);
        }
        Ok(hits)
    }

    /// Reclassify FTS5 syntax errors as `BadRequest` so the server returns a
    /// user-actionable error code instead of an opaque internal error.
    fn classify_search_error(e: rusqlite::Error, query: &str) -> DoogatError {
        let msg = e.to_string();
        if msg.contains("fts5: syntax error") || msg.contains("fts5: parse error") {
            DoogatError::BadRequest(format!("invalid search query: {query}"))
        } else {
            DoogatError::Sql(msg)
        }
    }

    fn map_search_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchResult> {
        let doogat_type: Option<String> = row.get(6)?;
        let doogat_type = doogat_type.filter(|s| !s.is_empty());
        Ok(SearchResult {
            id: row.get(0)?,
            title: row.get(1)?,
            path: row.get(2)?,
            snippet: row.get(3)?,
            rank: row.get(4)?,
            updated_at: row.get::<_, String>(5).unwrap_or_default(),
            tags: Vec::new(),
            doogat_type,
            fields: None,
            created_at: row.get(7)?,
        })
    }

    fn enrich_search_hits(&self, hits: &mut [SearchResult]) {
        if hits.is_empty() {
            return;
        }
        let ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
        let placeholders: String = (1..=ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");

        self.enrich_tags(hits, &ids, &placeholders);
        self.enrich_fields(hits, &ids, &placeholders);
        self.enrich_materialized_fields(hits);
    }

    /// Batch-fetch tags from `_ddb_tags` and attach to each hit.
    fn enrich_tags(&self, hits: &mut [SearchResult], ids: &[String], placeholders: &str) {
        let sql = format!(
            "SELECT doogat_id, tag FROM _ddb_tags WHERE doogat_id IN ({placeholders})"
        );
        let result = self.conn.prepare(&sql).and_then(|mut stmt| {
            let params: Vec<&dyn rusqlite::types::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
            let rows = stmt.query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut tag_map: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for row in rows.flatten() {
                tag_map.entry(row.0).or_default().push(row.1);
            }
            Ok(tag_map)
        });
        match result {
            Ok(mut tag_map) => {
                for hit in hits.iter_mut() {
                    if let Some(tags) = tag_map.remove(&hit.id) {
                        hit.tags = tags;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "enrich_search_hits: tags query failed");
            }
        }
    }

    /// Batch-fetch inline fields from `_ddb_fields` and attach to each hit.
    fn enrich_fields(&self, hits: &mut [SearchResult], ids: &[String], placeholders: &str) {
        let sql = format!(
            "SELECT doogat_id, key, value FROM _ddb_fields WHERE doogat_id IN ({placeholders})"
        );
        let result = self.conn.prepare(&sql).and_then(|mut stmt| {
            let params: Vec<&dyn rusqlite::types::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
            let rows = stmt.query_map(params.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            let mut field_map: std::collections::HashMap<
                String,
                std::collections::BTreeMap<String, String>,
            > = std::collections::HashMap::new();
            for row in rows.flatten() {
                if let Some(val) = row.2 {
                    field_map.entry(row.0).or_default().insert(row.1, val);
                }
            }
            Ok(field_map)
        });
        match result {
            Ok(mut field_map) => {
                for hit in hits.iter_mut() {
                    if let Some(fields) = field_map.remove(&hit.id) {
                        hit.fields = Some(fields);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "enrich_search_hits: fields query failed");
            }
        }
    }

    /// Supplement hits with fields from materialized type tables (body-zone columns).
    fn enrich_materialized_fields(&self, hits: &mut [SearchResult]) {
        let mut type_groups: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, hit) in hits.iter().enumerate() {
            if let Some(ref t) = hit.doogat_type {
                type_groups.entry(t.clone()).or_default().push(i);
            }
        }

        for (type_name, hit_indices) in &type_groups {
            let col_names = match self.materialized_column_names(type_name) {
                Some(cols) => cols,
                None => continue,
            };
            if col_names.is_empty() {
                continue;
            }
            self.merge_materialized_columns(hits, hit_indices, type_name, &col_names);
        }
    }

    /// Get non-core column names from a materialized type table via PRAGMA.
    fn materialized_column_names(&self, type_name: &str) -> Option<Vec<String>> {
        use super::materialize::is_core_column;

        let safe_name = escape_sql_ident(type_name);
        let pragma_sql = format!("PRAGMA table_info(\"{}\")", safe_name);
        match self.conn.prepare(&pragma_sql) {
            Ok(mut stmt) => {
                let cols = stmt
                    .query_map([], |row| row.get::<_, String>(1))
                    .ok()
                    .map(|rows| {
                        rows.flatten()
                            .filter(|name| !is_core_column(name))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(cols)
            }
            Err(e) => {
                tracing::warn!(error = %e, type_name, "enrich_search_hits: PRAGMA failed");
                None
            }
        }
    }

    fn merge_materialized_columns(
        &self,
        hits: &mut [SearchResult],
        hit_indices: &[usize],
        type_name: &str,
        col_names: &[String],
    ) {
        let field_map = match self.fetch_materialized_fields(hits, hit_indices, type_name, col_names)
        {
            Some(m) => m,
            None => return,
        };
        apply_fields_to_hits(hits, hit_indices, field_map);
    }

    fn fetch_materialized_fields(
        &self,
        hits: &[SearchResult],
        hit_indices: &[usize],
        type_name: &str,
        col_names: &[String],
    ) -> Option<std::collections::HashMap<String, std::collections::BTreeMap<String, String>>> {
        let safe_name = escape_sql_ident(type_name);
        let type_ids: Vec<&str> = hit_indices
            .iter()
            .map(|&i| hits[i].id.as_str())
            .collect();
        let placeholders: String = (1..=type_ids.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let col_select: String = col_names
            .iter()
            .map(|c| format!("\"{}\"", escape_sql_ident(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, {} FROM \"{}\" WHERE id IN ({})",
            col_select, safe_name, placeholders
        );

        let mut stmt = self.conn.prepare(&sql).ok()?;
        let params: Vec<&dyn rusqlite::types::ToSql> =
            type_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
        let rows = stmt
            .query_map(params.as_slice(), |row| {
                let id: String = row.get(0)?;
                let mut fields = std::collections::BTreeMap::new();
                for (col_idx, col_name) in col_names.iter().enumerate() {
                    if let Ok(Some(val)) = row.get::<_, Option<String>>(col_idx + 1) {
                        fields.insert(col_name.clone(), val);
                    }
                }
                Ok((id, fields))
            })
            .ok()?;

        let mut field_map = std::collections::HashMap::new();
        for row in rows.flatten() {
            if !row.1.is_empty() {
                field_map.insert(row.0, row.1);
            }
        }
        Some(field_map)
    }

    /// Return all tags with their usage counts, ordered by count descending then name ascending.
    pub fn list_tags(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tag, COUNT(*) as count FROM _ddb_tags GROUP BY tag ORDER BY count DESC, tag ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Find doogats by hierarchical tag prefix.
    pub fn by_tag(&self, prefix: &str) -> Result<Vec<String>> {
        let pattern = format!("{prefix}%");
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT doogat_id FROM _ddb_tags WHERE tag LIKE ?1")?;
        let ids = stmt.query_map(params![pattern], |row| row.get(0))?;
        let mut out = Vec::new();
        for id in ids {
            out.push(id?);
        }
        Ok(out)
    }

    /// Query individual tag-doogat associations with optional filters.
    pub fn query_tags(&self, filter: &TagQueryFilter) -> Result<Vec<TagEntry>> {
        let mut sql = String::from("SELECT doogat_id, tag, source FROM _ddb_tags");
        let mut conditions: Vec<String> = Vec::new();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(ref id) = filter.doogat_id_eq {
            params.push(id.clone().into());
            conditions.push(format!("doogat_id = ?{}", params.len()));
        }
        if let Some(ref ids) = filter.doogat_id_in {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let placeholders: Vec<String> = ids
                .iter()
                .map(|id| {
                    params.push(id.clone().into());
                    format!("?{}", params.len())
                })
                .collect();
            conditions.push(format!("doogat_id IN ({})", placeholders.join(",")));
        }
        if let Some(ref tag) = filter.tag_eq {
            params.push(tag.clone().into());
            conditions.push(format!("tag = ?{}", params.len()));
        }
        if let Some(ref substr) = filter.tag_contains {
            params.push(format!("%{substr}%").into());
            conditions.push(format!("tag LIKE ?{}", params.len()));
        }
        if let Some(ref tags) = filter.tag_in {
            if tags.is_empty() {
                return Ok(Vec::new());
            }
            let placeholders: Vec<String> = tags
                .iter()
                .map(|t| {
                    params.push(t.clone().into());
                    format!("?{}", params.len())
                })
                .collect();
            conditions.push(format!("tag IN ({})", placeholders.join(",")));
        }

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY doogat_id, tag");

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p as &dyn rusqlite::types::ToSql).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(TagEntry {
                doogat_id: row.get(0)?,
                tag: row.get(1)?,
                source: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

fn apply_fields_to_hits(
    hits: &mut [SearchResult],
    hit_indices: &[usize],
    mut field_map: std::collections::HashMap<String, std::collections::BTreeMap<String, String>>,
) {
    for &i in hit_indices {
        if let Some(new_fields) = field_map.remove(&hits[i].id) {
            match hits[i].fields.as_mut() {
                Some(existing) => existing.extend(new_fields),
                None => hits[i].fields = Some(new_fields),
            }
        }
    }
}
