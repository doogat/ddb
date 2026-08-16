//! Service tests for injected index and mirror ports.

use std::collections::{BTreeMap, HashMap};

use rusqlite::Connection;

use crate::app_contract::{
    ApplySchemaCommand, AppWarning, CreateCommand, UnregisteredTypePolicy, UpdateCommand,
    REINDEX_SKIPPED_FILES,
};
use crate::error::{DoogatError, Result};
use crate::git_ops::GitRepo;
use crate::traits::{DoogatIndex, DoogatSource, GitHistory, IndexPort, NoopMirror, SqlBackend};
use crate::types::{
    BrokenSequence, ConsistencyWarning, LinkDensityEntry, OrphanDoogat, PaginatedSearchResult,
    ParsedDoogat, QueryValue, RebuildReport, RecentDoogat, SearchFilters, SearchResult,
    SequenceInfo, SequenceNode, StaleDoogat, Suggestion, TableSchema, TagEntry, TagQueryFilter,
    UnlinkedMention,
};

use super::DoogatService;

/// Mock index with canned path and timestamp lookups. It holds an in-memory
/// SQLite connection only to satisfy the transitional `sql_conn()` accessor on
/// `SqlBackend` — the test never queries it, so no real index logic runs.
struct MockIndex {
    conn: Connection,
    resolve_to: String,
    updated_at: Option<String>,
    rebuild_report: Option<RebuildReport>,
}

impl MockIndex {
    fn new(resolve_to: &str, updated_at: Option<&str>) -> Self {
        Self {
            conn: Connection::open_in_memory().unwrap(),
            resolve_to: resolve_to.to_string(),
            updated_at: updated_at.map(str::to_string),
            rebuild_report: None,
        }
    }

    /// Configure the `RebuildReport` (and its `ConsistencyWarning`s, if any)
    /// that `rebuild_if_stale` returns, so tests can drive `ensure_fresh`'s
    /// reindex-warning path deterministically.
    fn with_rebuild_report(mut self, report: RebuildReport) -> Self {
        self.rebuild_report = Some(report);
        self
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
        Ok(self.rebuild_report.clone())
    }
    fn rebuild(&self, _repo: &impl DoogatSource) -> Result<RebuildReport> {
        Ok(RebuildReport::default())
    }
    fn locked_rebuild(&self, _repo: &impl DoogatSource) -> Result<RebuildReport> {
        Ok(RebuildReport::default())
    }
    fn locked_explicit_rebuild(
        &self,
        _repo: &impl DoogatSource,
        _strict: bool,
    ) -> Result<RebuildReport> {
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
    fn recent_doogats(&self, _days: u32, _type_filter: Option<&str>) -> Result<Vec<RecentDoogat>> {
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

    repo.commit_file(
        "ddb/mock.md",
        "---\ntitle: Mock Doc\n---\nMock body content",
        "seed mock doc",
    )
    .unwrap();

    let mock = MockIndex::new("ddb/mock.md", Some("20260101000000"));
    let svc = DoogatService::from_parts(repo, mock, Box::new(NoopMirror), tmp.path().to_path_buf());

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

// ── ensure_fresh reindex-warning propagation (background reindex warnings
// must reach the `create`/`update`/`apply_schema` AppOutput envelopes) ──────

/// Build a `DoogatService` over a fresh git repo (with one seeded doogat at
/// `ddb/mock.md`, so `update`'s `resolve_path` -> `read_file` has real
/// content to read) and a `MockIndex` whose `rebuild_if_stale` returns
/// `rebuild_report`.
fn seeded_mock_service(
    rebuild_report: Option<RebuildReport>,
) -> (tempfile::TempDir, DoogatService<GitRepo, MockIndex>) {
    let tmp = tempfile::TempDir::new().unwrap();
    GitRepo::init(tmp.path()).unwrap();
    let repo = GitRepo::open(tmp.path()).unwrap();
    repo.commit_file(
        "ddb/mock.md",
        "---\ntitle: Original Title\n---\nBody content",
        "seed mock doc",
    )
    .unwrap();

    let mut mock = MockIndex::new("ddb/mock.md", None);
    if let Some(report) = rebuild_report {
        mock = mock.with_rebuild_report(report);
    }
    let svc = DoogatService::from_parts(repo, mock, Box::new(NoopMirror), tmp.path().to_path_buf());
    (tmp, svc)
}

/// A `RebuildReport` carrying `count` skipped-file warnings. `count > 1`
/// proves a facade summarizes to exactly ONE `AppWarning`, not one warning
/// per skipped file.
fn rebuild_report_with_warnings(count: usize) -> RebuildReport {
    let warnings = (0..count)
        .map(|i| ConsistencyWarning::UnreadableFile {
            path: format!("ddb/bad-{i}.md"),
            error: "permission denied".to_string(),
        })
        .collect();
    RebuildReport {
        warnings,
        ..Default::default()
    }
}

fn reindex_warning_count(warnings: &[AppWarning]) -> usize {
    warnings
        .iter()
        .filter(|w| w.code == REINDEX_SKIPPED_FILES)
        .count()
}

fn create_cmd(title: Option<&str>, doogat_type: Option<&str>, policy: UnregisteredTypePolicy) -> CreateCommand {
    CreateCommand {
        title: title.map(str::to_string),
        body: None,
        tags: vec![],
        doogat_type: doogat_type.map(str::to_string),
        fields: BTreeMap::new(),
        on_conflict: crate::types::ConflictAction::Error,
        unregistered_type_policy: policy,
    }
}

fn update_cmd(title: &str) -> UpdateCommand {
    UpdateCommand {
        id: "ignored-id".to_string(),
        title: Some(title.to_string()),
        tags: None,
        doogat_type: None,
        body: None,
        fields: BTreeMap::new(),
        unset_fields: vec![],
    }
}

#[test]
fn create_facade_includes_exactly_one_reindex_warning_when_files_are_skipped() {
    // A background reindex that skipped 2 files must summarize to exactly
    // ONE AppWarning coded REINDEX_SKIPPED_FILES, not one warning per file.
    let (_tmp, svc) = seeded_mock_service(Some(rebuild_report_with_warnings(2)));

    let output = svc
        .create(create_cmd(
            Some("New Doogat"),
            None,
            UnregisteredTypePolicy::Strict,
        ))
        .expect("create must return Ok");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        1,
        "create must summarize a multi-file skip into exactly one \
         REINDEX_SKIPPED_FILES warning, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn create_facade_emits_no_reindex_warning_when_nothing_was_skipped() {
    let (_tmp, svc) = seeded_mock_service(Some(RebuildReport::default()));

    let output = svc
        .create(create_cmd(
            Some("Clean Doogat"),
            None,
            UnregisteredTypePolicy::Strict,
        ))
        .expect("create must return Ok");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        0,
        "a reindex with no warnings must not add a REINDEX_SKIPPED_FILES entry, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn create_facade_preserves_baseonly_warning_alongside_reindex_warning() {
    // The BaseOnly-unregistered-type early return already emits its own
    // UNREGISTERED_TYPE_BASE_ONLY warning (`try_baseonly_unregistered`). A
    // reindex that also skipped files must ADD its warning there too, not
    // replace the pre-existing one -- this is the additive-merge contract
    // exercised on a facade return point other than the main happy path.
    let (_tmp, svc) = seeded_mock_service(Some(rebuild_report_with_warnings(1)));

    let output = svc
        .create(create_cmd(
            None,
            Some("unregistered_widget"),
            UnregisteredTypePolicy::BaseOnly,
        ))
        .expect("create with an unregistered type under BaseOnly policy must return Ok");

    let codes: Vec<&str> = output.warnings.iter().map(|w| w.code).collect();
    assert!(
        codes.contains(&"UNREGISTERED_TYPE_BASE_ONLY"),
        "the pre-existing BaseOnly warning must survive alongside the reindex warning, got: {codes:?}"
    );
    assert_eq!(
        reindex_warning_count(&output.warnings),
        1,
        "the reindex warning must be ADDITIVE alongside the pre-existing warning, got: {codes:?}"
    );
}

#[test]
fn update_facade_includes_exactly_one_reindex_warning_when_files_are_skipped() {
    let (_tmp, svc) = seeded_mock_service(Some(rebuild_report_with_warnings(2)));

    let output = svc
        .update(update_cmd("Updated Title"))
        .expect("update must return Ok");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        1,
        "update must summarize a multi-file skip into exactly one \
         REINDEX_SKIPPED_FILES warning, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn update_facade_emits_no_reindex_warning_when_nothing_was_skipped() {
    let (_tmp, svc) = seeded_mock_service(Some(RebuildReport::default()));

    let output = svc
        .update(update_cmd("Updated Title"))
        .expect("update must return Ok");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        0,
        "a reindex with no warnings must not add a REINDEX_SKIPPED_FILES entry, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn skip_stale_check_produces_no_reindex_warning_even_when_mock_would_report_some() {
    let (_tmp, mut svc) = seeded_mock_service(Some(rebuild_report_with_warnings(2)));
    svc.set_skip_stale_check(true);

    let output = svc
        .update(update_cmd("Updated Title"))
        .expect("update must return Ok even with skip_stale_check enabled");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        0,
        "skip_stale_check must suppress reindex warnings even though the mock \
         would have reported some, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn apply_schema_facade_includes_exactly_one_reindex_warning_when_files_are_skipped() {
    // An empty `types:` list produces an empty plan (no ops, no
    // SCHEMA_UNSUPPORTED_CHANGE warnings), which exercises apply_schema's
    // early "empty-plan" return point without needing a real SqlEngine-backed
    // index -- exactly the return point most likely to be missed since it is
    // one of THREE separate returns in this facade.
    let (_tmp, mut svc) = seeded_mock_service(Some(rebuild_report_with_warnings(2)));

    let output = svc
        .apply_schema(ApplySchemaCommand {
            schema_doc: "types: []".to_string(),
            dry_run: false,
            allow_destructive: false,
        })
        .expect("apply_schema with an empty type list must return Ok");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        1,
        "apply_schema must summarize a multi-file skip into exactly one \
         REINDEX_SKIPPED_FILES warning, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn apply_schema_facade_emits_no_reindex_warning_when_nothing_was_skipped() {
    let (_tmp, mut svc) = seeded_mock_service(Some(RebuildReport::default()));

    let output = svc
        .apply_schema(ApplySchemaCommand {
            schema_doc: "types: []".to_string(),
            dry_run: false,
            allow_destructive: false,
        })
        .expect("apply_schema with an empty type list must return Ok");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        0,
        "a reindex with no warnings must not add a REINDEX_SKIPPED_FILES entry, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn apply_schema_dry_run_return_point_also_carries_the_reindex_warning() {
    // `apply_schema` has THREE return points sharing one `warnings` vec built
    // at the top of the function: dry_run, empty-plan, and the main path. The
    // empty-plan return is covered above; this covers the dry_run return
    // (checked BEFORE empty-plan), which an impl that merged the reindex
    // warning into only one of the three return points could still miss.
    let (_tmp, mut svc) = seeded_mock_service(Some(rebuild_report_with_warnings(2)));

    let output = svc
        .apply_schema(ApplySchemaCommand {
            schema_doc: "types: []".to_string(),
            dry_run: true,
            allow_destructive: false,
        })
        .expect("dry_run apply_schema with an empty type list must return Ok");

    assert_eq!(
        reindex_warning_count(&output.warnings),
        1,
        "apply_schema's dry_run return point must also summarize a multi-file \
         skip into exactly one REINDEX_SKIPPED_FILES warning, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}
