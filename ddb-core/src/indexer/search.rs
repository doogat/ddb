use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::search_query::{self, SearchExpr};
use crate::types::{
    PaginatedSearchResult, SearchFieldOp, SearchFilters, SearchResult, TagEntry, TagQueryFilter,
};

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

    /// Parse the query and extract negation info.
    /// Returns `None` if no negation handling needed (original behavior).
    /// Returns `Some((positive_fts_query, negation_sql_clauses, negation_params))`
    /// where positive_fts_query is None for all-negative queries.
    fn build_negation_plan(&self, query: &str) -> Option<(Option<String>, Vec<String>, Vec<String>)> {
        let ast = search_query::parse(query)?;
        let (positive, negatives) = search_query::extract_negations(ast);
        if negatives.is_empty() {
            return None;
        }

        let pos_query = positive.as_ref().map(search_query::to_fts_query);

        let mut neg_clauses = Vec::new();
        let mut neg_params = Vec::new();
        for neg in &negatives {
            match neg {
                SearchExpr::FieldEquals { field, value } if field == "tag" => {
                    neg_clauses.push("tag".to_string());
                    neg_params.push(value.clone());
                }
                other => {
                    neg_clauses.push("fts".to_string());
                    neg_params.push(search_query::to_fts_query(other));
                }
            }
        }

        Some((pos_query, neg_clauses, neg_params))
    }

    /// Generate SQL exclusion clauses for negated terms and collect params.
    fn resolve_negation_clauses(
        &self,
        neg_clauses: &[String],
        neg_params: &[String],
        all_params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    ) -> String {
        let mut sql = String::new();
        for (clause_type, value) in neg_clauses.iter().zip(neg_params.iter()) {
            all_params.push(Box::new(value.clone()));
            let idx = all_params.len();
            match clause_type.as_str() {
                "tag" => {
                    sql.push_str(&format!(
                        " AND z.id NOT IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?{idx})"
                    ));
                }
                _ => {
                    sql.push_str(&format!(
                        " AND z.id NOT IN (\
                         SELECT z2.id FROM _ddb_fts \
                         JOIN doogats z2 ON z2.rowid = _ddb_fts.rowid \
                         WHERE _ddb_fts MATCH ?{idx})"
                    ));
                }
            }
        }
        sql
    }

    /// Re-index filter SQL clauses for all-negative queries where there is
    /// no ?1 FTS MATCH param. Copies filter_params into all_params and
    /// returns adjusted SQL with correct parameter indices.
    fn reindex_filter_sql(
        filter_sql: &str,
        filter_params: &[String],
        all_params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    ) -> String {
        if filter_params.is_empty() {
            return String::new();
        }
        let base = all_params.len();
        for p in filter_params {
            all_params.push(Box::new(p.clone()));
        }
        // Two-pass replacement to avoid overlapping index corruption.
        // Pass 1: old indices -> unique markers
        let mut result = filter_sql.to_string();
        for i in (0..filter_params.len()).rev() {
            let old = format!("?{}", i + 2);
            let marker = format!("__P{i}__");
            result = result.replace(&old, &marker);
        }
        // Pass 2: markers -> new indices
        for i in 0..filter_params.len() {
            let marker = format!("__P{i}__");
            let new = format!("?{}", base + i + 1);
            result = result.replace(&marker, &new);
        }
        result
    }

    fn build_filter_clauses(&self, filters: &SearchFilters) -> (Vec<String>, Vec<String>) {
        use super::materialize::is_core_column;

        let mut clauses = Vec::new();
        let mut params = Vec::new();
        let mut idx = 2; // ?1 is always the FTS query

        if let Some(ref types) = filters.types {
            if !types.is_empty() {
                let placeholders: Vec<String> =
                    types.iter().map(|_| { let p = format!("?{idx}"); idx += 1; p }).collect();
                clauses.push(format!("AND z.type IN ({})", placeholders.join(", ")));
                params.extend(types.clone());
            }
        }

        if let Some(ref tag) = filters.tag {
            clauses.push(format!(
                "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?{idx})"
            ));
            params.push(tag.clone());
            idx += 1;
        }

        if let Some(ref where_filters) = filters.where_filters {
            // Determine candidate type tables for materialized column lookup
            let candidate_tables: Vec<String> = if let Some(ref types) = filters.types {
                types.clone()
            } else {
                self.conn
                    .prepare(
                        "SELECT name FROM sqlite_master WHERE type='table' \
                         AND name NOT LIKE '_ddb%' AND name != 'doogats' \
                         AND name NOT LIKE 'sqlite_%'",
                    )
                    .and_then(|mut stmt| {
                        stmt.query_map([], |row| row.get::<_, String>(0))
                            .map(|rows| rows.flatten().collect::<Vec<String>>())
                    })
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|name| {
                        // Exclude junction tables (they have no `id` column)
                        self.conn
                            .prepare(&format!(
                                "PRAGMA table_info(\"{}\")",
                                Self::escape_sql_ident(name)
                            ))
                            .and_then(|mut stmt| {
                                stmt.query_map([], |row| row.get::<_, String>(1))
                                    .map(|rows| rows.flatten().any(|col| col == "id"))
                            })
                            .unwrap_or(false)
                    })
                    .collect::<Vec<String>>()
            };

            for wf in where_filters {
                // Route "tag" field to _ddb_tags table
                if wf.field == "tag" {
                    match &wf.op {
                        SearchFieldOp::Eq(val) => {
                            clauses.push(format!(
                                "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?{idx})"
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::Contains(val) => {
                            clauses.push(format!(
                                "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag LIKE '%' || ?{idx} || '%')"
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::In(vals) => {
                            if vals.is_empty() {
                                clauses.push("AND 0".to_string());
                            } else {
                                let placeholders: Vec<String> = vals.iter().map(|_| {
                                    let p = format!("?{idx}");
                                    idx += 1;
                                    p
                                }).collect();
                                clauses.push(format!(
                                    "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag IN ({}))",
                                    placeholders.join(", ")
                                ));
                                params.extend(vals.clone());
                            }
                        }
                    }
                    continue;
                }

                // Route core doogat columns directly to the z (doogats) alias
                if is_core_column(&wf.field) {
                    let safe_col = Self::escape_sql_ident(&wf.field);
                    match &wf.op {
                        SearchFieldOp::Eq(val) => {
                            clauses.push(format!(
                                "AND z.\"{}\" = ?{idx}", safe_col
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::Contains(val) => {
                            clauses.push(format!(
                                "AND z.\"{}\" LIKE '%' || ?{idx} || '%'", safe_col
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::In(vals) => {
                            if vals.is_empty() {
                                clauses.push("AND 0".to_string());
                            } else {
                                let placeholders: Vec<String> = vals.iter().map(|_| {
                                    let p = format!("?{idx}");
                                    idx += 1;
                                    p
                                }).collect();
                                clauses.push(format!(
                                    "AND z.\"{}\" IN ({})", safe_col, placeholders.join(", ")
                                ));
                                params.extend(vals.clone());
                            }
                        }
                    }
                    continue;
                }

                // Find which candidate tables have this field as a non-core column
                let mut tables_with_field: Vec<String> = Vec::new();
                for table in &candidate_tables {
                    let has_col = self
                        .conn
                        .prepare(&format!(
                            "PRAGMA table_info(\"{}\")",
                            Self::escape_sql_ident(table)
                        ))
                        .and_then(|mut stmt| {
                            stmt.query_map([], |row| row.get::<_, String>(1))
                                .map(|rows| {
                                    rows.flatten()
                                        .any(|col| !is_core_column(&col) && col == wf.field)
                                })
                        })
                        .unwrap_or(false);
                    if has_col {
                        tables_with_field.push(table.clone());
                    }
                }

                if tables_with_field.is_empty() {
                    // Check for junction tables ({type}_{field}) before
                    // falling back to _ddb_fields.
                    let safe_field = Self::escape_sql_ident(&wf.field);
                    let mut junction_tables: Vec<String> = Vec::new();
                    for table in &candidate_tables {
                        let jt_name = format!("{}_{}", table, wf.field);
                        let exists: bool = self
                            .conn
                            .query_row(
                                "SELECT COUNT(*) > 0 FROM sqlite_master \
                                 WHERE type='table' AND name=?1",
                                [&jt_name],
                                |row| row.get(0),
                            )
                            .unwrap_or(false);
                        if exists {
                            junction_tables.push(table.clone());
                        }
                    }

                    if !junction_tables.is_empty() {
                        match &wf.op {
                            SearchFieldOp::Eq(val) => {
                                let ph = format!("?{idx}");
                                idx += 1;
                                let subs: Vec<String> = junction_tables
                                    .iter()
                                    .map(|t| {
                                        let st = Self::escape_sql_ident(t);
                                        let jt = format!("{st}_{safe_field}");
                                        format!(
                                            "SELECT \"{st}_id\" FROM \"{jt}\" \
                                             WHERE \"{safe_field}_id\" = {ph}"
                                        )
                                    })
                                    .collect();
                                clauses.push(format!(
                                    "AND z.id IN ({})",
                                    subs.join(" UNION ")
                                ));
                                params.push(val.clone());
                            }
                            SearchFieldOp::Contains(val) => {
                                let ph = format!("?{idx}");
                                idx += 1;
                                let subs: Vec<String> = junction_tables
                                    .iter()
                                    .map(|t| {
                                        let st = Self::escape_sql_ident(t);
                                        let jt = format!("{st}_{safe_field}");
                                        format!(
                                            "SELECT jt.\"{st}_id\" FROM \"{jt}\" jt \
                                             JOIN doogats d ON d.id = jt.\"{safe_field}_id\" \
                                             WHERE d.title LIKE '%' || {ph} || '%'"
                                        )
                                    })
                                    .collect();
                                clauses.push(format!(
                                    "AND z.id IN ({})",
                                    subs.join(" UNION ")
                                ));
                                params.push(val.clone());
                            }
                            SearchFieldOp::In(vals) => {
                                if vals.is_empty() {
                                    clauses.push("AND 0".to_string());
                                } else {
                                    let phs: Vec<String> = vals
                                        .iter()
                                        .map(|_| {
                                            let p = format!("?{idx}");
                                            idx += 1;
                                            p
                                        })
                                        .collect();
                                    let in_list = phs.join(", ");
                                    let subs: Vec<String> = junction_tables
                                        .iter()
                                        .map(|t| {
                                            let st = Self::escape_sql_ident(t);
                                            let jt = format!("{st}_{safe_field}");
                                            format!(
                                                "SELECT \"{st}_id\" FROM \"{jt}\" \
                                                 WHERE \"{safe_field}_id\" IN ({in_list})"
                                            )
                                        })
                                        .collect();
                                    clauses.push(format!(
                                        "AND z.id IN ({})",
                                        subs.join(" UNION ")
                                    ));
                                    params.extend(vals.clone());
                                }
                            }
                        }
                    } else {
                        // Fallback: use _ddb_fields key-value store (two params)
                        match &wf.op {
                            SearchFieldOp::Eq(val) => {
                                clauses.push(format!(
                                    "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value = ?{})",
                                    idx, idx + 1
                                ));
                                params.push(wf.field.clone());
                                params.push(val.clone());
                                idx += 2;
                            }
                            SearchFieldOp::Contains(val) => {
                                clauses.push(format!(
                                    "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value LIKE '%' || ?{} || '%')",
                                    idx, idx + 1
                                ));
                                params.push(wf.field.clone());
                                params.push(val.clone());
                                idx += 2;
                            }
                            SearchFieldOp::In(vals) => {
                                if vals.is_empty() {
                                    clauses.push("AND 0".to_string());
                                } else {
                                    let key_idx = idx;
                                    idx += 1;
                                    let placeholders: Vec<String> = vals.iter().map(|_| {
                                        let p = format!("?{idx}");
                                        idx += 1;
                                        p
                                    }).collect();
                                    clauses.push(format!(
                                        "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value IN ({}))",
                                        key_idx, placeholders.join(", ")
                                    ));
                                    params.push(wf.field.clone());
                                    params.extend(vals.clone());
                                }
                            }
                        }
                    } // end junction_tables else (fallback)
                } else {
                    // Resolve against materialized type table(s)
                    let safe_col = Self::escape_sql_ident(&wf.field);

                    match &wf.op {
                        SearchFieldOp::In(vals) => {
                            if vals.is_empty() {
                                clauses.push("AND 0".to_string());
                            } else {
                                let placeholders: Vec<String> = vals.iter().map(|_| {
                                    let p = format!("?{idx}");
                                    idx += 1;
                                    p
                                }).collect();
                                let in_list = placeholders.join(", ");

                                let subqueries: Vec<String> = tables_with_field
                                    .iter()
                                    .map(|t| {
                                        let safe_table = Self::escape_sql_ident(t);
                                        format!(
                                            "SELECT id FROM \"{}\" WHERE \"{}\" IN ({})",
                                            safe_table, safe_col, in_list
                                        )
                                    })
                                    .collect();

                                let combined = subqueries.join(" UNION ");
                                clauses.push(format!("AND z.id IN ({combined})"));
                                params.extend(vals.clone());
                            }
                        }
                        SearchFieldOp::Contains(val) => {
                            // For REFERENCES columns, the materialized column stores raw IDs.
                            // Prefer junction table JOIN so Contains matches on referenced title.
                            let jt_tables: Vec<String> = tables_with_field
                                .iter()
                                .filter(|t| {
                                    let jt_name = format!("{}_{}", t, wf.field);
                                    self.conn
                                        .query_row(
                                            "SELECT COUNT(*) > 0 FROM sqlite_master \
                                             WHERE type='table' AND name=?1",
                                            [&jt_name],
                                            |row| row.get::<_, bool>(0),
                                        )
                                        .unwrap_or(false)
                                })
                                .cloned()
                                .collect();

                            if !jt_tables.is_empty() {
                                let ph = format!("?{idx}");
                                idx += 1;
                                let subs: Vec<String> = jt_tables
                                    .iter()
                                    .map(|t| {
                                        let st = Self::escape_sql_ident(t);
                                        let jt = format!("{st}_{safe_col}");
                                        format!(
                                            "SELECT jt.\"{st}_id\" FROM \"{jt}\" jt \
                                             JOIN doogats d ON d.id = jt.\"{safe_col}_id\" \
                                             WHERE d.title LIKE '%' || {ph} || '%'"
                                        )
                                    })
                                    .collect();
                                clauses.push(format!("AND z.id IN ({})", subs.join(" UNION ")));
                                params.push(val.clone());
                            } else {
                                let param_placeholder = format!("?{idx}");
                                idx += 1;
                                let subqueries: Vec<String> = tables_with_field
                                    .iter()
                                    .map(|t| {
                                        let safe_table = Self::escape_sql_ident(t);
                                        format!(
                                            "SELECT id FROM \"{}\" WHERE \"{}\" LIKE '%' || {} || '%'",
                                            safe_table, safe_col, param_placeholder
                                        )
                                    })
                                    .collect();
                                clauses.push(format!("AND z.id IN ({})", subqueries.join(" UNION ")));
                                params.push(val.clone());
                            }
                        }
                        SearchFieldOp::Eq(val) => {
                            let param_placeholder = format!("?{idx}");
                            idx += 1;
                            let subqueries: Vec<String> = tables_with_field
                                .iter()
                                .map(|t| {
                                    let safe_table = Self::escape_sql_ident(t);
                                    format!(
                                        "SELECT id FROM \"{}\" WHERE \"{}\" = {}",
                                        safe_table, safe_col, param_placeholder
                                    )
                                })
                                .collect();
                            let combined = subqueries.join(" UNION ");
                            clauses.push(format!("AND z.id IN ({combined})"));
                            params.push(val.clone());
                        }
                    }
                }
            }
        }

        (clauses, params)
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

    /// Escape a SQL identifier by doubling embedded double-quotes.
    fn escape_sql_ident(name: &str) -> String {
        name.replace('"', "\"\"")
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

        // Batch-fetch tags
        {
            let sql = format!(
                "SELECT doogat_id, tag FROM _ddb_tags WHERE doogat_id IN ({placeholders})"
            );
            match self.conn.prepare(&sql).and_then(|mut stmt| {
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
            }) {
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

        // Batch-fetch fields from _ddb_fields (frontmatter extras + inline fields)
        {
            let sql = format!(
                "SELECT doogat_id, key, value FROM _ddb_fields WHERE doogat_id IN ({placeholders})"
            );
            match self.conn.prepare(&sql).and_then(|mut stmt| {
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
            }) {
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

        // Supplement with fields from materialized type tables (covers body-zone columns)
        {
            use super::materialize::is_core_column;

            // Group hit indices by type
            let mut type_groups: std::collections::HashMap<String, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, hit) in hits.iter().enumerate() {
                if let Some(ref t) = hit.doogat_type {
                    type_groups.entry(t.clone()).or_default().push(i);
                }
            }

            for (type_name, hit_indices) in &type_groups {
                let safe_name = Self::escape_sql_ident(type_name);
                // Get type-specific column names from the materialized table
                let pragma_sql = format!("PRAGMA table_info(\"{}\")", safe_name);
                let col_names: Vec<String> = match self.conn.prepare(&pragma_sql) {
                    Ok(mut stmt) => stmt
                        .query_map([], |row| row.get::<_, String>(1))
                        .ok()
                        .map(|rows| {
                            rows.flatten()
                                .filter(|name| !is_core_column(name))
                                .collect()
                        })
                        .unwrap_or_default(),
                    Err(e) => {
                        tracing::warn!(error = %e, type_name, "enrich_search_hits: PRAGMA failed");
                        continue;
                    }
                };
                if col_names.is_empty() {
                    continue;
                }

                let type_ids: Vec<&str> = hit_indices
                    .iter()
                    .map(|&i| hits[i].id.as_str())
                    .collect();
                let type_placeholders: String = (1..=type_ids.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let col_select: String = col_names
                    .iter()
                    .map(|c| format!("\"{}\"", Self::escape_sql_ident(c)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT id, {} FROM \"{}\" WHERE id IN ({})",
                    col_select, safe_name, type_placeholders
                );

                if let Ok(mut stmt) = self.conn.prepare(&sql) {
                    let params: Vec<&dyn rusqlite::types::ToSql> =
                        type_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
                    if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
                        let id: String = row.get(0)?;
                        let mut fields = std::collections::BTreeMap::new();
                        for (col_idx, col_name) in col_names.iter().enumerate() {
                            if let Ok(Some(val)) = row.get::<_, Option<String>>(col_idx + 1) {
                                fields.insert(col_name.clone(), val);
                            }
                        }
                        Ok((id, fields))
                    }) {
                        let mut field_map: std::collections::HashMap<
                            String,
                            std::collections::BTreeMap<String, String>,
                        > = std::collections::HashMap::new();
                        for row in rows.flatten() {
                            if !row.1.is_empty() {
                                field_map.insert(row.0, row.1);
                            }
                        }
                        for &i in hit_indices {
                            if let Some(new_fields) = field_map.remove(&hits[i].id) {
                                match hits[i].fields.as_mut() {
                                    Some(existing) => existing.extend(new_fields),
                                    None => hits[i].fields = Some(new_fields),
                                }
                            }
                        }
                    }
                }
            }
        }
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
