use crate::app_contract::{summarize_reindex_warnings, AppOutput, AppSearchResult, SearchCommand};
use crate::error::Result;
use crate::parser;
use crate::types::{
    ListFilter, PaginatedSearchResult, ParsedDoogat, QueryValue, RebuildReport, SearchFilters,
    SearchResult, TagEntry, TagQueryFilter, TypedListQuery,
};

use crate::traits::{GitBackend, IndexPort};

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
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

    /// App facade entrypoint: run a filtered, paginated search from a
    /// `SearchCommand`. Hits come back exactly as the index returns them;
    /// `total` is the index's full match count, not the page length.
    /// `warnings` carries at most one `REINDEX_SKIPPED_FILES` entry,
    /// summarizing any files the up-front freshness reindex had to skip.
    pub fn search_command(&self, cmd: SearchCommand) -> Result<AppOutput<AppSearchResult>> {
        let reindex_warnings = self.ensure_fresh()?;
        let filters = SearchFilters {
            types: cmd.types,
            tag: cmd.tag,
            where_filters: None,
        };
        let result =
            self.search_paginated_filtered(&cmd.query, cmd.page.limit, cmd.page.offset, &filters)?;
        Ok(AppOutput {
            value: AppSearchResult {
                items: result.hits,
                total: result.total_count,
                query: cmd.query,
            },
            warnings: summarize_reindex_warnings(reindex_warnings)
                .into_iter()
                .collect(),
        })
    }

    pub fn reindex(&self) -> Result<RebuildReport> {
        self.index.locked_explicit_rebuild(&self.repo, false)
    }

    pub fn reindex_strict(&self) -> Result<RebuildReport> {
        self.index.locked_explicit_rebuild(&self.repo, true)
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
                match self.repo.read_file(path) {
                    Ok(content) => match parser::parse(&content, path) {
                        Ok(mut parsed) => {
                            parsed.updated_at = updated_at;
                            doogats.push(parsed);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, path, "list_doogats_filtered: parse failed, row dropped");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, path, "list_doogats_filtered: file read failed, row dropped");
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
    pub fn aggregate_query(&self, sql: &str, params: &[QueryValue]) -> Result<Vec<String>> {
        self.ensure_fresh()?;
        let rows = self.index.query_raw_with_query_values(sql, params)?;
        Ok(rows.into_iter().next().unwrap_or_default())
    }

    /// Execute a raw SQL query with params, returning all result rows.
    pub fn query_raw_with_params(
        &self,
        sql: &str,
        params: &[QueryValue],
    ) -> Result<Vec<Vec<String>>> {
        self.ensure_fresh()?;
        self.index.query_raw_with_query_values(sql, params)
    }

    pub fn query_raw_with_columns(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        self.ensure_fresh()?;
        self.index.query_raw_with_columns(sql)
    }

    /// Query a materialized type table with WHERE/ORDER/LIMIT, returning parsed doogats.
    pub fn typed_filtered_list(&self, query: &TypedListQuery) -> Result<Vec<ParsedDoogat>> {
        self.ensure_fresh()?;
        let sql = build_typed_list_sql(query);
        let rows = self
            .index
            .query_raw_with_query_values(&sql, &query.params)?;
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.first().map(|s| s.as_str()))
            .collect();
        let updated_map = match self.index.lookup_updated_at_batch(&ids) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "typed_filtered_list: updated_at batch lookup failed, timestamps omitted");
                Default::default()
            }
        };
        let mut doogats = Vec::new();
        for row in rows {
            let Some(id) = row.first() else { continue };
            let path = match self.index.resolve_path(id) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, id, "typed_filtered_list: path resolution failed, row dropped");
                    continue;
                }
            };
            let content = match self.repo.read_file(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, path, "typed_filtered_list: file read failed, row dropped");
                    continue;
                }
            };
            let mut parsed = match parser::parse(&content, &path) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, path, "typed_filtered_list: parse failed, row dropped");
                    continue;
                }
            };
            parsed.updated_at = updated_map.get(id.as_str()).cloned();
            doogats.push(parsed);
        }
        Ok(doogats)
    }
}

/// Sortable columns on the doogats table.
pub const SORTABLE_COLUMNS: &[&str] = &["id", "title", "date", "type", "updated_at"];

fn build_typed_list_sql(query: &TypedListQuery) -> String {
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
    let where_clause = build_where_clause(&conditions);
    let order = query.order_sql.as_deref().unwrap_or("id DESC");
    let limit_clause = build_limit_clause(query.limit, query.offset);
    let group_by = match &query.distinct {
        Some(col) => format!(" GROUP BY \"{}\"", col.replace('"', "\"\"")),
        None => String::new(),
    };
    format!(
        "SELECT id FROM \"{}\"{where_clause}{group_by} ORDER BY {order}{limit_clause}",
        query.table_name
    )
}

fn build_limit_clause(limit: Option<i64>, offset: Option<i64>) -> String {
    match (limit, offset) {
        (Some(l), Some(o)) => format!(" LIMIT {l} OFFSET {o}"),
        (Some(l), None) => format!(" LIMIT {l}"),
        (None, Some(o)) => format!(" LIMIT -1 OFFSET {o}"),
        (None, None) => String::new(),
    }
}

fn build_filter_conditions(filter: &ListFilter) -> Vec<String> {
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
    conditions
}

fn build_order_clause(filter: &ListFilter) -> String {
    match filter
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
    }
}

fn build_where_clause(conditions: &[String]) -> String {
    if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    }
}

fn build_filtered_sql(filter: &ListFilter) -> String {
    let conditions = build_filter_conditions(filter);
    let where_clause = build_where_clause(&conditions);
    let order_clause = build_order_clause(filter);
    let limit_clause = build_limit_clause(filter.limit, filter.offset);
    format!("SELECT z.id, z.path, z.updated_at FROM doogats z{where_clause}{order_clause}{limit_clause}")
}

/// Tests for the `search_command` app-contract facade. The facade must hand
/// back the index's hits untouched, apply `types`/`tag` filters, honour the
/// page, echo the query, and surface background-reindex skips as warnings.
#[cfg(test)]
mod tests {
    use crate::app_contract::{
        AppOutput, AppSearchResult, PageRequest, SearchCommand, REINDEX_SKIPPED_FILES,
    };
    use crate::service::DoogatService;
    use crate::types::{SearchFilters, SearchResult, Value};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn fresh_svc() -> (TempDir, DoogatService) {
        let tmp = TempDir::new().unwrap();
        let svc = DoogatService::init(tmp.path()).expect("init repo");
        svc.reindex().expect("initial reindex");
        (tmp, svc)
    }

    fn cmd(
        query: &str,
        limit: usize,
        offset: usize,
        types: Option<Vec<&str>>,
        tag: Option<&str>,
    ) -> SearchCommand {
        SearchCommand {
            query: query.to_string(),
            page: PageRequest { limit, offset },
            types: types.map(|t| t.into_iter().map(str::to_string).collect()),
            tag: tag.map(str::to_string),
        }
    }

    fn run(svc: &DoogatService, cmd: SearchCommand) -> AppOutput<AppSearchResult> {
        svc.search_command(cmd).expect("search_command must return Ok")
    }

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|t| t.to_string()).collect()
    }

    fn sorted_ids(items: &[SearchResult]) -> Vec<String> {
        let mut ids: Vec<String> = items.iter().map(|h| h.id.clone()).collect();
        ids.sort();
        ids
    }

    /// All 10 `SearchResult` fields must survive the facade unchanged.
    fn assert_hit_unmodified(actual: &SearchResult, expected: &SearchResult) {
        assert_eq!(actual.id, expected.id, "id changed");
        assert_eq!(actual.title, expected.title, "title changed");
        assert_eq!(actual.path, expected.path, "path changed");
        assert_eq!(actual.snippet, expected.snippet, "snippet changed");
        assert_eq!(actual.rank, expected.rank, "rank changed");
        assert_eq!(actual.updated_at, expected.updated_at, "updated_at changed");
        assert_eq!(actual.tags, expected.tags, "tags changed");
        assert_eq!(actual.doogat_type, expected.doogat_type, "doogat_type changed");
        assert_eq!(actual.fields, expected.fields, "fields changed");
        assert_eq!(actual.created_at, expected.created_at, "created_at changed");
    }

    #[test]
    fn search_command_hits_carry_every_index_field_unmodified() {
        let (_tmp, svc) = fresh_svc();
        let mut extra = BTreeMap::new();
        extra.insert("color".to_string(), Value::String("teal".to_string()));
        let created = svc
            .create_doogat_with_extra(
                "Fixture Title",
                &tags(&["alpha", "beta"]),
                Some("recipe"),
                "fixtureterm body text",
                extra,
            )
            .expect("create fixture doogat");
        let id = created.meta.id.clone().expect("created doogat has an id").0;

        let output = run(&svc, cmd("fixtureterm", 10, 0, None, None));
        let direct = svc
            .search_paginated_filtered("fixtureterm", 10, 0, &SearchFilters::default())
            .expect("direct paginated search");

        assert_eq!(output.value.items.len(), 1, "exactly the fixture must match");
        assert_eq!(direct.hits.len(), 1, "direct search must also find the fixture");
        let hit = &output.value.items[0];

        // Concrete values from the fixture.
        assert_eq!(hit.id, id);
        assert_eq!(hit.title, "Fixture Title");
        assert_eq!(hit.path, created.path);
        let mut hit_tags = hit.tags.clone();
        hit_tags.sort();
        assert_eq!(hit_tags, tags(&["alpha", "beta"]));
        assert_eq!(hit.doogat_type.as_deref(), Some("recipe"));
        assert_eq!(
            hit.fields
                .as_ref()
                .and_then(|f| f.get("color"))
                .map(String::as_str),
            Some("teal"),
            "frontmatter extra must reach the hit's fields, got: {:?}",
            hit.fields
        );
        assert!(!hit.snippet.is_empty(), "snippet must be populated");
        assert!(!hit.updated_at.is_empty(), "updated_at must be populated");
        assert!(created.meta.date.is_some(), "fixture must carry a date");
        assert_eq!(hit.created_at, created.meta.date, "created_at must be the doogat's date");

        // Every field identical to what the index returned.
        assert_hit_unmodified(hit, &direct.hits[0]);

        assert_eq!(output.value.total, 1);
        assert_eq!(output.value.query, "fixtureterm");
    }

    #[test]
    fn search_command_total_comes_from_index_total_count_not_page_length() {
        let (_tmp, svc) = fresh_svc();
        for title in ["One", "Two", "Three"] {
            svc.create_doogat(title, &[], None, "pageterm content")
                .expect("create page fixture");
        }

        let first = run(&svc, cmd("pageterm", 2, 0, None, None));
        let second = run(&svc, cmd("pageterm", 2, 2, None, None));

        assert_eq!(first.value.items.len(), 2, "limit 2 must cap the first page");
        assert_eq!(second.value.items.len(), 1, "offset 2 leaves one hit");
        assert_eq!(first.value.total, 3, "total is the full match count");
        assert_eq!(second.value.total, 3, "total is the full match count");

        let direct_first = svc
            .search_paginated_filtered("pageterm", 2, 0, &SearchFilters::default())
            .unwrap();
        let direct_second = svc
            .search_paginated_filtered("pageterm", 2, 2, &SearchFilters::default())
            .unwrap();
        assert_eq!(
            first.value.items.iter().map(|h| &h.id).collect::<Vec<_>>(),
            direct_first.hits.iter().map(|h| &h.id).collect::<Vec<_>>(),
            "page.limit/page.offset must be forwarded unchanged"
        );
        assert_eq!(
            second.value.items.iter().map(|h| &h.id).collect::<Vec<_>>(),
            direct_second.hits.iter().map(|h| &h.id).collect::<Vec<_>>(),
            "page.limit/page.offset must be forwarded unchanged"
        );
    }

    #[test]
    fn search_command_echoes_the_command_query() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Echo", &[], None, "echoterm content")
            .expect("create fixture");

        let output = run(&svc, cmd("echoterm", 10, 0, None, None));

        assert_eq!(output.value.query, "echoterm");
        assert_eq!(output.value.items.len(), 1);
    }

    /// Fixture: three doogats sharing `filterterm`, differing in type and tags.
    /// Returns (soup_id, cake_id, note_id).
    fn seed_filter_fixtures(svc: &DoogatService) -> (String, String, String) {
        let soup = svc
            .create_doogat("Soup", &tags(&["dinner"]), Some("recipe"), "filterterm soup")
            .expect("create soup");
        let cake = svc
            .create_doogat(
                "Cake",
                &tags(&["dinner", "sweet"]),
                Some("dessert"),
                "filterterm cake",
            )
            .expect("create cake");
        let note = svc
            .create_doogat("Note", &[], None, "filterterm note")
            .expect("create note");
        (soup, cake, note)
    }

    #[test]
    fn search_command_without_filters_returns_every_match() {
        let (_tmp, svc) = fresh_svc();
        let (soup, cake, note) = seed_filter_fixtures(&svc);

        let output = run(&svc, cmd("filterterm", 10, 0, None, None));

        let mut expected = vec![soup, cake, note];
        expected.sort();
        assert_eq!(sorted_ids(&output.value.items), expected);
        assert_eq!(output.value.total, 3);
    }

    #[test]
    fn search_command_types_filter_keeps_only_listed_types() {
        let (_tmp, svc) = fresh_svc();
        let (soup, _cake, _note) = seed_filter_fixtures(&svc);

        let output = run(&svc, cmd("filterterm", 10, 0, Some(vec!["recipe"]), None));

        assert_eq!(sorted_ids(&output.value.items), vec![soup]);
        assert_eq!(output.value.total, 1, "total must respect the types filter");
        assert!(
            output
                .value
                .items
                .iter()
                .all(|h| h.doogat_type.as_deref() == Some("recipe")),
            "every hit must be of a listed type"
        );
    }

    #[test]
    fn search_command_tag_filter_keeps_only_tagged_doogats() {
        let (_tmp, svc) = fresh_svc();
        let (_soup, cake, _note) = seed_filter_fixtures(&svc);

        let output = run(&svc, cmd("filterterm", 10, 0, None, Some("sweet")));

        assert_eq!(sorted_ids(&output.value.items), vec![cake]);
        assert_eq!(output.value.total, 1, "total must respect the tag filter");
    }

    #[test]
    fn search_command_combines_types_and_tag_filters() {
        let (_tmp, svc) = fresh_svc();
        let (soup, cake, _note) = seed_filter_fixtures(&svc);

        let both = run(
            &svc,
            cmd("filterterm", 10, 0, Some(vec!["recipe", "dessert"]), Some("dinner")),
        );
        let mut expected = vec![soup.clone(), cake.clone()];
        expected.sort();
        assert_eq!(sorted_ids(&both.value.items), expected);
        assert_eq!(both.value.total, 2);

        let narrowed = run(
            &svc,
            cmd("filterterm", 10, 0, Some(vec!["recipe", "dessert"]), Some("sweet")),
        );
        assert_eq!(
            sorted_ids(&narrowed.value.items),
            vec![cake],
            "the tag filter must still apply alongside the types filter"
        );
        assert_eq!(narrowed.value.total, 1);

        // cake is tagged `dinner` but is a dessert: the types filter must
        // still apply alongside the tag filter.
        let typed = run(
            &svc,
            cmd("filterterm", 10, 0, Some(vec!["recipe"]), Some("dinner")),
        );
        assert_eq!(
            sorted_ids(&typed.value.items),
            vec![soup],
            "the types filter must still apply alongside the tag filter"
        );
        assert_eq!(typed.value.total, 1);
    }

    #[test]
    fn search_command_emits_no_warning_when_fresh_check_skips_nothing() {
        let (_tmp, svc) = fresh_svc();
        svc.create_doogat("Clean", &[], None, "cleanterm content")
            .expect("create fixture");

        let output = run(&svc, cmd("cleanterm", 10, 0, None, None));

        assert!(
            output.warnings.is_empty(),
            "a clean index must produce no warnings, got: {:?}",
            output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
        );
        assert_eq!(output.value.items.len(), 1);
    }

    #[test]
    fn search_command_surfaces_one_reindex_skipped_files_warning_when_fresh_check_skips_a_file() {
        let (_tmp, svc) = fresh_svc();
        let visible = svc
            .create_doogat("Visible", &[], None, "warnterm content")
            .expect("create visible doogat");

        // Commit a malformed doogat behind the index's back: the next
        // ensure_fresh reindexes and must skip it with a warning.
        let malformed_path = "ddb/20290402000000.md";
        svc.repo
            .commit_file(
                malformed_path,
                "---\n: invalid yaml [\n---\nwarnterm",
                "add a malformed doogat",
            )
            .expect("commit malformed doogat");

        let output = run(&svc, cmd("warnterm", 10, 0, None, None));

        let codes: Vec<&str> = output.warnings.iter().map(|w| w.code).collect();
        assert_eq!(
            codes,
            vec![REINDEX_SKIPPED_FILES],
            "the skipped file must surface as exactly one REINDEX_SKIPPED_FILES warning"
        );
        assert!(
            output.warnings[0].message.contains(malformed_path),
            "warning message must name the skipped file, got: {}",
            output.warnings[0].message
        );
        assert_eq!(
            sorted_ids(&output.value.items),
            vec![visible],
            "the skip must not drop the valid hit"
        );
    }
}
