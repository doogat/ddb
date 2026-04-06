use crate::error::Result;
use crate::parser;
use crate::types::{
    ListFilter, PaginatedSearchResult, ParsedDoogat, RebuildReport, SearchFilters, SearchResult,
    TagEntry, TagQueryFilter, TypedListQuery,
};

use super::DoogatService;

impl DoogatService {
    // ── Search ──────────────────────────────────────────────────────────

    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.ensure_fresh()?;
        self.index.search(query)
    }

    pub fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.ensure_fresh()?;
        self.index.search_paginated(query, limit, offset)
    }

    pub fn search_paginated_filtered(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        filters: &SearchFilters,
    ) -> Result<PaginatedSearchResult> {
        self.ensure_fresh()?;
        self.index
            .search_paginated_filtered(query, limit, offset, filters)
    }

    pub fn reindex(&self) -> Result<RebuildReport> {
        self.index.rebuild(&self.repo)
    }

    pub fn rebuild_if_stale(&self) -> Result<()> {
        self.ensure_fresh()?;
        Ok(())
    }

    // ── Filtered Queries ─────────────────────────────────────────────────

    /// Query doogats matching filter criteria, returning parsed doogats.
    pub fn list_doogats_filtered(&self, filter: &ListFilter) -> Result<Vec<ParsedDoogat>> {
        self.ensure_fresh()?;
        let sql = build_filtered_sql(filter);
        let rows = self.index.query_raw(&sql)?;
        let mut doogats = Vec::new();
        for row in rows {
            if row.len() >= 2 {
                let path = &row[1];
                let updated_at = row.get(2).cloned();
                if let Ok(content) = self.repo.read_file(path) {
                    if let Ok(mut parsed) = parser::parse(&content, path) {
                        parsed.updated_at = updated_at;
                        doogats.push(parsed);
                    }
                }
            }
        }
        Ok(doogats)
    }

    /// Count doogats matching filter criteria.
    pub fn count_doogats_filtered(&self, filter: &ListFilter) -> Result<i64> {
        self.ensure_fresh()?;
        let count_filter = ListFilter {
            limit: None,
            offset: None,
            sort_field: None,
            sort_desc: None,
            ..filter.clone()
        };
        let select_sql = build_filtered_sql(&count_filter);
        let count_sql = format!("SELECT COUNT(*) FROM ({select_sql})");
        let rows = self.index.query_raw(&count_sql)?;
        let count = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        Ok(count)
    }

    /// List all tags with usage counts, ordered by count descending.
    pub fn list_tags(&self) -> Result<Vec<(String, i64)>> {
        self.ensure_fresh()?;
        self.index.list_tags()
    }

    /// Query individual tag-doogat associations with optional filters.
    pub fn query_tags(&self, filter: &TagQueryFilter) -> Result<Vec<TagEntry>> {
        self.ensure_fresh()?;
        self.index.query_tags(filter)
    }

    /// Execute a raw SQL query with params, returning the first result row.
    pub fn aggregate_query(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<String>> {
        self.ensure_fresh()?;
        let rows = self.index.query_raw_with_params(sql, params)?;
        Ok(rows.into_iter().next().unwrap_or_default())
    }

    /// Execute a raw SQL query with params, returning all result rows.
    pub fn query_raw_with_params(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<Vec<String>>> {
        self.ensure_fresh()?;
        self.index.query_raw_with_params(sql, params)
    }

    pub fn query_raw_with_columns(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        self.ensure_fresh()?;
        self.index.query_raw_with_columns(sql)
    }

    /// Query a materialized type table with WHERE/ORDER/LIMIT, returning parsed doogats.
    pub fn typed_filtered_list(&self, query: &TypedListQuery) -> Result<Vec<ParsedDoogat>> {
        self.ensure_fresh()?;

        let mut conditions = Vec::new();
        if !query.where_sql.is_empty() {
            conditions.push(query.where_sql.to_string());
        }
        if let Some(t) = &query.tag {
            conditions.push(format!(
                "id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = '{}')",
                t.replace('\'', "''")
            ));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let order = query.order_sql.as_deref().unwrap_or("id DESC");
        let limit_clause = match (query.limit, query.offset) {
            (Some(l), Some(o)) => format!(" LIMIT {l} OFFSET {o}"),
            (Some(l), None) => format!(" LIMIT {l}"),
            (None, Some(o)) => format!(" LIMIT -1 OFFSET {o}"),
            (None, None) => String::new(),
        };

        let group_by = match &query.distinct {
            Some(col) => format!(" GROUP BY \"{}\"", col.replace('"', "\"\"")),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id FROM \"{}\"{where_clause}{group_by} ORDER BY {order}{limit_clause}",
            query.table_name
        );

        let rows = self.index.query_raw_with_params(&sql, &query.params)?;
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.first().map(|s| s.as_str()))
            .collect();
        let updated_map = self.index.lookup_updated_at_batch(&ids).unwrap_or_default();
        let mut doogats = Vec::new();
        for row in rows {
            if let Some(id) = row.first() {
                if let Ok(path) = self.index.resolve_path(id) {
                    if let Ok(content) = self.repo.read_file(&path) {
                        if let Ok(mut parsed) = parser::parse(&content, &path) {
                            parsed.updated_at = updated_map.get(id.as_str()).cloned();
                            doogats.push(parsed);
                        }
                    }
                }
            }
        }
        Ok(doogats)
    }
}

/// Sortable columns on the doogats table.
pub const SORTABLE_COLUMNS: &[&str] = &["id", "title", "date", "type", "updated_at"];

/// Build SQL query with filters for doogat listing.
fn build_filtered_sql(filter: &ListFilter) -> String {
    let mut conditions = Vec::new();

    if let Some(t) = filter.doogat_type.as_deref() {
        conditions.push(format!("z.type = '{}'", t.replace('\'', "''")));
    }
    if let Some(t) = filter.tag.as_deref() {
        conditions.push(format!(
            "z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = '{}')",
            t.replace('\'', "''")
        ));
    }
    if let Some(bl) = filter.backlinks_of.as_deref() {
        conditions.push(format!(
            "z.id IN (SELECT source_id FROM _ddb_links WHERE target_path = '{}')",
            bl.replace('\'', "''")
        ));
    }
    for (key, value) in &filter.field_filters {
        conditions.push(format!(
            "z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = '{}' AND value = '{}')",
            key.replace('\'', "''"),
            value.replace('\'', "''")
        ));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let limit_clause = match (filter.limit, filter.offset) {
        (Some(l), Some(o)) => format!(" LIMIT {l} OFFSET {o}"),
        (Some(l), None) => format!(" LIMIT {l}"),
        (None, Some(o)) => format!(" LIMIT -1 OFFSET {o}"),
        (None, None) => String::new(),
    };

    let order_clause = match filter
        .sort_field
        .as_deref()
        .filter(|f| SORTABLE_COLUMNS.contains(f))
    {
        Some(field) => {
            let default_desc = matches!(field, "date" | "id");
            let dir = if filter.sort_desc.unwrap_or(default_desc) {
                "DESC"
            } else {
                "ASC"
            };
            if field == "id" {
                format!(" ORDER BY z.id {dir}")
            } else {
                format!(" ORDER BY z.{field} {dir}, z.id DESC")
            }
        }
        None => " ORDER BY z.date DESC, z.id DESC".to_string(),
    };

    format!("SELECT z.id, z.path, z.updated_at FROM doogats z{where_clause}{order_clause}{limit_clause}")
}
