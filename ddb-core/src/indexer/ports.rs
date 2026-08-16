//! Trait implementations for the concrete SQLite `Index`.
//!
//! The `SqlBackend`, `DoogatIndex`, `IndexPort`, and `TypedMaterializationPort` impls are
//! pass-throughs that delegate to the inherent methods on `Index` (see `indexer/mod.rs`).
//! Split out of `mod.rs` per PRD 00156 to keep the module under the 800-line cap.

use crate::error::Result;
use crate::types::{PaginatedSearchResult, ParsedDoogat, SearchResult};

use super::Index;

/// Pass-through trait impl for `SqlBackend`. Each method body delegates to
/// the inherent method of the same name on `Index`. Rust's method-resolution
/// rules pick the inherent method over the trait method when called via
/// `self.<method>(...)`, so these bodies are not self-recursive. The compiler
/// also catches accidental recursion via the on-by-default
/// `unconditional_recursion` lint if an inherent method is ever removed
/// without updating the trait body. PRD 00134 cycle-1 review C1 task #8.
impl crate::traits::SqlBackend for Index {
    fn sql_conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    fn query_raw_with_columns(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        self.query_raw_with_columns(sql)
    }

    fn rematerialize_type(
        &self,
        type_name: &str,
        source: &dyn crate::traits::DoogatSource,
    ) -> Result<()> {
        self.rematerialize_type(type_name, source)
    }

    fn materialize_single(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        parsed: &crate::types::ParsedDoogat,
    ) -> Result<()> {
        self.materialize_single(schema, id, parsed)
    }

    fn populate_junction_tables(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        parsed: &crate::types::ParsedDoogat,
    ) -> Result<()> {
        self.populate_junction_tables(schema, id, parsed)
    }

    fn sync_junction_tables_for_columns(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        parsed: &crate::types::ParsedDoogat,
        changed_cols: &[&str],
    ) -> Result<()> {
        self.sync_junction_tables_for_columns(schema, id, parsed, changed_cols)
    }

    fn type_uses_folder(&self, type_name: &str, source: &dyn crate::traits::DoogatSource) -> bool {
        self.type_uses_folder(type_name, source)
    }

    fn backlinks_by_target(
        &self,
        target_id: &str,
        target_path: &str,
    ) -> Result<Vec<(String, String)>> {
        self.backlinks_by_target(target_id, target_path)
    }

    fn check_restrict_blocks_delete(
        &self,
        source: &dyn crate::traits::DoogatSource,
        deleted_id: &str,
    ) -> Result<()> {
        self.check_restrict_blocks_delete(source, deleted_id)
    }
}

/// Pass-through trait impl for `DoogatIndex`. Same dispatch contract as the
/// `SqlBackend` impl above: bodies delegate to inherent methods on `Index`,
/// recursion would be caught by `unconditional_recursion`. PRD 00134
/// cycle-1 review C1 task #8.
impl crate::traits::DoogatIndex for Index {
    fn index_doogat(&self, doogat: &ParsedDoogat) -> Result<()> {
        self.index_doogat(doogat)
    }

    fn remove_doogat(&self, id: &str) -> Result<()> {
        self.remove_doogat(id)
    }

    fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.search(query)
    }

    fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.search_paginated(query, limit, offset)
    }

    fn resolve_path(&self, id: &str) -> Result<String> {
        self.resolve_path(id)
    }

    fn query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        self.query_raw(sql)
    }

    fn find_typedef_path(&self, type_name: &str) -> Result<Option<String>> {
        self.find_typedef_path(type_name)
    }

    fn execute_sql(&self, sql: &str, params: &[&str]) -> Result<usize> {
        self.execute_sql(sql, params)
    }
}

/// Pass-through trait impl for `IndexPort`. Methods here are the operations not
/// already inherited from the `DoogatIndex` and `SqlBackend` supertraits.
impl crate::traits::IndexPort for Index {
    fn rebuild_if_stale(
        &self,
        repo: &impl crate::traits::DoogatSource,
    ) -> Result<Option<crate::types::RebuildReport>> {
        self.rebuild_if_stale(repo)
    }

    fn rebuild(
        &self,
        repo: &impl crate::traits::DoogatSource,
    ) -> Result<crate::types::RebuildReport> {
        self.rebuild(repo)
    }

    fn locked_rebuild(
        &self,
        repo: &impl crate::traits::DoogatSource,
    ) -> Result<crate::types::RebuildReport> {
        self.locked_rebuild(repo)
    }

    fn locked_explicit_rebuild(
        &self,
        repo: &impl crate::traits::DoogatSource,
        strict: bool,
    ) -> Result<crate::types::RebuildReport> {
        self.locked_explicit_rebuild(repo, strict)
    }

    fn is_stale(&self, repo: &impl crate::traits::DoogatSource) -> Result<bool> {
        self.is_stale(repo)
    }

    fn store_head(&self, head: &str) -> Result<()> {
        self.store_head(head)
    }

    fn lookup_updated_at(&self, id: &str) -> Result<Option<String>> {
        self.lookup_updated_at(id)
    }

    fn lookup_updated_at_batch(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        self.lookup_updated_at_batch(ids)
    }

    fn query_raw_with_query_values(
        &self,
        sql: &str,
        params: &[crate::types::QueryValue],
    ) -> Result<Vec<Vec<String>>> {
        self.query_raw_with_query_values(sql, params)
    }

    fn load_all_typedefs(
        &self,
        repo: &dyn crate::traits::DoogatSource,
    ) -> std::collections::HashMap<String, crate::types::TableSchema> {
        self.load_all_typedefs(repo)
    }

    fn collect_cascade_children(
        &self,
        repo: &dyn crate::traits::DoogatSource,
        deleted_id: &str,
    ) -> Result<Vec<(String, String)>> {
        self.collect_cascade_children(repo, deleted_id)
    }

    fn cascade_junction_cleanup(
        &self,
        repo: &dyn crate::traits::DoogatSource,
        target_type: &str,
        deleted_id: &str,
    ) -> Result<()> {
        self.cascade_junction_cleanup(repo, target_type, deleted_id)
    }

    fn list_tags(&self) -> Result<Vec<(String, i64)>> {
        self.list_tags()
    }

    fn query_tags(
        &self,
        filter: &crate::types::TagQueryFilter,
    ) -> Result<Vec<crate::types::TagEntry>> {
        self.query_tags(filter)
    }

    fn unlinked_mentions(&self, target_id: &str) -> Result<Vec<crate::types::UnlinkedMention>> {
        self.unlinked_mentions(target_id)
    }

    fn suggest_links(
        &self,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::Suggestion>> {
        self.suggest_links(source_id, limit)
    }

    fn stale_doogats(
        &self,
        repo: &(impl crate::traits::DoogatSource + crate::traits::GitHistory),
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::StaleDoogat>> {
        self.stale_doogats(repo, type_filter)
    }

    fn orphan_doogats(&self, type_filter: Option<&str>) -> Result<Vec<crate::types::OrphanDoogat>> {
        self.orphan_doogats(type_filter)
    }

    fn recent_doogats(
        &self,
        days: u32,
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::RecentDoogat>> {
        self.recent_doogats(days, type_filter)
    }

    fn link_density(
        &self,
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::LinkDensityEntry>> {
        self.link_density(type_filter)
    }

    fn sequence_tree(
        &self,
        id: &str,
        max_depth: usize,
    ) -> Result<Vec<(crate::types::SequenceNode, usize)>> {
        self.sequence_tree(id, max_depth)
    }

    fn sequence_breadcrumb(&self, id: &str) -> Result<Vec<crate::types::SequenceNode>> {
        self.sequence_breadcrumb(id)
    }

    fn broken_sequences(&self) -> Result<Vec<crate::types::BrokenSequence>> {
        self.broken_sequences()
    }

    fn sequence_info(&self, id: &str) -> Result<crate::types::SequenceInfo> {
        self.sequence_info(id)
    }

    fn sequence_children(&self, id: &str) -> Result<Vec<crate::types::SequenceNode>> {
        self.sequence_children(id)
    }

    fn backlinks(&self, target_path: &str) -> Result<Vec<String>> {
        self.backlinks(target_path)
    }

    fn backlinking_doogat_paths(&self, target: &str) -> Result<Vec<(String, String)>> {
        self.backlinking_doogat_paths(target)
    }

    fn resurrected_doogats(&self) -> Result<Vec<(String, String)>> {
        self.resurrected_doogats()
    }

    fn broken_backlinks(&self) -> Result<Vec<(String, String)>> {
        self.broken_backlinks()
    }

    fn infer_schema(
        &self,
        type_name: &str,
        repo: &(impl crate::traits::DoogatSource + ?Sized),
    ) -> Result<crate::types::TableSchema> {
        self.infer_schema(type_name, repo)
    }

    fn query_raw_with_params(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<Vec<String>>> {
        self.query_raw_with_params(sql, params)
    }

    fn search_paginated_filtered(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        filters: &crate::types::SearchFilters,
    ) -> Result<PaginatedSearchResult> {
        self.search_paginated_filtered(query, limit, offset, filters)
    }
}

/// Pass-through trait impl for `TypedMaterializationPort`.
impl crate::traits::TypedMaterializationPort for Index {
    fn infer_schema(
        &self,
        type_name: &str,
        repo: &(impl crate::traits::DoogatSource + ?Sized),
    ) -> Result<crate::types::TableSchema> {
        self.infer_schema(type_name, repo)
    }

    fn load_all_typedefs(
        &self,
        repo: &dyn crate::traits::DoogatSource,
    ) -> std::collections::HashMap<String, crate::types::TableSchema> {
        self.load_all_typedefs(repo)
    }
}
