//! Service unit test that injects a mock index port instead of real SQLite.
//!
//! Proves PRD 00142 success criterion 4: a meaningful service test runs against
//! a mock `IndexPort` (no real SQLite index logic) plus a `NoopMirror`, with git
//! storage supplied by a throwaway `GitRepo`. `get_doogat_parsed` is exercised
//! end to end: it routes `resolve_path` and `lookup_updated_at` through the
//! injected port and merges them with the real parser over real git content.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::{DoogatError, Result};
use crate::git_ops::GitRepo;
use crate::traits::{
    DoogatIndex, DoogatSource, GitHistory, IndexPort, NoopMirror, SqlBackend,
};
use crate::types::{
    BrokenSequence, LinkDensityEntry, OrphanDoogat, PaginatedSearchResult, ParsedDoogat,
    QueryValue, RebuildReport, RecentDoogat, SearchFilters, SearchResult, SequenceInfo,
    SequenceNode, StaleDoogat, Suggestion, TableSchema, TagEntry, TagQueryFilter, UnlinkedMention,
};

use super::DoogatService;

/// Mock index that returns canned values for the two methods the test exercises
/// (`resolve_path`, `lookup_updated_at`) and inert results elsewhere. It holds an
/// in-memory SQLite connection only to satisfy the transitional `sql_conn()`
/// accessor on `SqlBackend`; the test never queries it, so no real index logic
/// runs. Methods whose return type has no `Default` and that the test does not
/// call return an explicit error rather than a fabricated value.
struct MockIndex {
    conn: Connection,
    resolve_to: String,
    updated_at: Option<String>,
}

impl MockIndex {
    fn new(resolve_to: &str, updated_at: Option<&str>) -> Self {
        Self {
            conn: Connection::open_in_memory().unwrap(),
            resolve_to: resolve_to.to_string(),
            updated_at: updated_at.map(str::to_string),
        }
    }
}

fn mock_unsupported() -> DoogatError {
    DoogatError::Validation("mock index: method not exercised by this test".into())
}

impl DoogatIndex for MockIndex {
    fn index_doogat(&self, _doogat: &ParsedDoogat) -> Result<()> {
        Ok(())
    }
    fn remove_doogat(&self, _id: &str) -> Result<()> {
        Ok(())
    }
    fn search(&self, _query: &str) -> Result<Vec<SearchResult>> {
        Ok(Vec::new())
    }
    fn search_paginated(
        &self,
        _query: &str,
        _limit: usize,
        _offset: usize,
    ) -> Result<PaginatedSearchResult> {
        Err(mock_unsupported())
    }
    fn resolve_path(&self, _id: &str) -> Result<String> {
        Ok(self.resolve_to.clone())
    }
    fn query_raw(&self, _sql: &str) -> Result<Vec<Vec<String>>> {
        Ok(Vec::new())
    }
    fn find_typedef_path(&self, _type_name: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn execute_sql(&self, _sql: &str, _params: &[&str]) -> Result<usize> {
        Ok(0)
    }
}

impl SqlBackend for MockIndex {
    fn sql_conn(&self) -> &Connection {
        &self.conn
    }
    fn query_raw_with_columns(&self, _sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        Ok((Vec::new(), Vec::new()))
    }
    fn rematerialize_type(&self, _type_name: &str, _source: &dyn DoogatSource) -> Result<()> {
        Ok(())
    }
    fn materialize_single(
        &self,
        _schema: &TableSchema,
        _id: &str,
        _parsed: &ParsedDoogat,
    ) -> Result<()> {
        Ok(())
    }
    fn populate_junction_tables(
        &self,
        _schema: &TableSchema,
        _id: &str,
        _parsed: &ParsedDoogat,
    ) -> Result<()> {
        Ok(())
    }
    fn sync_junction_tables_for_columns(
        &self,
        _schema: &TableSchema,
        _id: &str,
        _parsed: &ParsedDoogat,
        _changed_cols: &[&str],
    ) -> Result<()> {
        Ok(())
    }
    fn type_uses_folder(&self, _type_name: &str, _source: &dyn DoogatSource) -> bool {
        false
    }
    fn backlinks_by_target(
        &self,
        _target_id: &str,
        _target_path: &str,
    ) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }
    fn check_restrict_blocks_delete(
        &self,
        _source: &dyn DoogatSource,
        _deleted_id: &str,
    ) -> Result<()> {
        Ok(())
    }
}

impl IndexPort for MockIndex {
    fn rebuild_if_stale(&self, _repo: &impl DoogatSource) -> Result<Option<RebuildReport>> {
        Ok(None)
    }
    fn rebuild(&self, _repo: &impl DoogatSource) -> Result<RebuildReport> {
        Ok(RebuildReport::default())
    }
    fn is_stale(&self, _repo: &impl DoogatSource) -> Result<bool> {
        Ok(false)
    }
    fn store_head(&self, _head: &str) -> Result<()> {
        Ok(())
    }
    fn lookup_updated_at(&self, _id: &str) -> Result<Option<String>> {
        Ok(self.updated_at.clone())
    }
    fn lookup_updated_at_batch(&self, _ids: &[&str]) -> Result<HashMap<String, String>> {
        Ok(HashMap::new())
    }
    fn query_raw_with_query_values(
        &self,
        _sql: &str,
        _params: &[QueryValue],
    ) -> Result<Vec<Vec<String>>> {
        Ok(Vec::new())
    }
    fn load_all_typedefs(&self, _repo: &dyn DoogatSource) -> HashMap<String, TableSchema> {
        HashMap::new()
    }
    fn collect_cascade_children(
        &self,
        _repo: &dyn DoogatSource,
        _deleted_id: &str,
    ) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }
    fn cascade_junction_cleanup(
        &self,
        _repo: &dyn DoogatSource,
        _target_type: &str,
        _deleted_id: &str,
    ) -> Result<()> {
        Ok(())
    }
    fn list_tags(&self) -> Result<Vec<(String, i64)>> {
        Ok(Vec::new())
    }
    fn query_tags(&self, _filter: &TagQueryFilter) -> Result<Vec<TagEntry>> {
        Ok(Vec::new())
    }
    fn unlinked_mentions(&self, _target_id: &str) -> Result<Vec<UnlinkedMention>> {
        Ok(Vec::new())
    }
    fn suggest_links(&self, _source_id: &str, _limit: usize) -> Result<Vec<Suggestion>> {
        Ok(Vec::new())
    }
    fn stale_doogats(
        &self,
        _repo: &(impl DoogatSource + GitHistory),
        _type_filter: Option<&str>,
    ) -> Result<Vec<StaleDoogat>> {
        Ok(Vec::new())
    }
    fn orphan_doogats(&self, _type_filter: Option<&str>) -> Result<Vec<OrphanDoogat>> {
        Ok(Vec::new())
    }
    fn recent_doogats(
        &self,
        _days: u32,
        _type_filter: Option<&str>,
    ) -> Result<Vec<RecentDoogat>> {
        Ok(Vec::new())
    }
    fn link_density(&self, _type_filter: Option<&str>) -> Result<Vec<LinkDensityEntry>> {
        Ok(Vec::new())
    }
    fn sequence_tree(&self, _id: &str, _max_depth: usize) -> Result<Vec<(SequenceNode, usize)>> {
        Ok(Vec::new())
    }
    fn sequence_breadcrumb(&self, _id: &str) -> Result<Vec<SequenceNode>> {
        Ok(Vec::new())
    }
    fn broken_sequences(&self) -> Result<Vec<BrokenSequence>> {
        Ok(Vec::new())
    }
    fn sequence_info(&self, _id: &str) -> Result<SequenceInfo> {
        Err(mock_unsupported())
    }
    fn sequence_children(&self, _id: &str) -> Result<Vec<SequenceNode>> {
        Ok(Vec::new())
    }
    fn backlinks(&self, _target_path: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn backlinking_doogat_paths(&self, _target: &str) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }
    fn resurrected_doogats(&self) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }
    fn broken_backlinks(&self) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }
    fn infer_schema(
        &self,
        _type_name: &str,
        _repo: &(impl DoogatSource + ?Sized),
    ) -> Result<TableSchema> {
        Err(mock_unsupported())
    }
    fn query_raw_with_params(
        &self,
        _sql: &str,
        _params: &[rusqlite::types::Value],
    ) -> Result<Vec<Vec<String>>> {
        Ok(Vec::new())
    }
    fn search_paginated_filtered(
        &self,
        _query: &str,
        _limit: usize,
        _offset: usize,
        _filters: &SearchFilters,
    ) -> Result<PaginatedSearchResult> {
        Err(mock_unsupported())
    }
}

#[test]
fn get_doogat_parsed_runs_against_mock_index() {
    let tmp = tempfile::TempDir::new().unwrap();
    GitRepo::init(tmp.path()).unwrap();
    let repo = GitRepo::open(tmp.path()).unwrap();

    // Real git content the service will read and parse.
    repo.commit_file(
        "ddb/mock.md",
        "---\ntitle: Mock Doc\n---\nMock body content",
        "seed mock doc",
    )
    .unwrap();

    // Inject the mock index + no-op mirror; no real SQLite index is constructed.
    let mock = MockIndex::new("ddb/mock.md", Some("20260101000000"));
    let svc =
        DoogatService::from_parts(repo, mock, Box::new(NoopMirror), tmp.path().to_path_buf());

    // The id is irrelevant: the mock resolves every id to "ddb/mock.md", which a
    // real SQLite index could not do — proving the injected port is the seam.
    let parsed = svc.get_doogat_parsed("ignored-id").unwrap();

    assert_eq!(
        parsed.meta.title.as_deref(),
        Some("Mock Doc"),
        "title comes from the real parser over git content the mock pointed at"
    );
    assert_eq!(
        parsed.updated_at.as_deref(),
        Some("20260101000000"),
        "updated_at must come from the injected index port's lookup_updated_at, not real SQLite"
    );
}
