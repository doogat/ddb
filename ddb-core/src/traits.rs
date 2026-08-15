use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;
use crate::types::{
    CommitHash, ConflictFile, DiffKind, MergeResult, PaginatedSearchResult, ParsedDoogat,
    RepoConfig, ResolvedFile, SearchResult, TableSchema,
};

/// Read-only access to doogat storage.
pub trait DoogatSource {
    fn list_doogats(&self) -> Result<Vec<String>>;
    fn read_file(&self, path: &str) -> Result<String>;
    fn head_oid(&self) -> Result<CommitHash>;
    /// Diff two tree OIDs, returning changed paths with their change kind.
    /// Returns `Err` if either OID is unreachable (e.g. after gc).
    fn diff_paths(&self, old_oid: &str, new_oid: &str) -> Result<Vec<(DiffKind, String)>>;

    /// Read multiple files, returning per-file results.
    /// Default implementation calls `read_file` in a loop.
    fn read_files_batch(&self, paths: &[String]) -> Result<Vec<(String, Result<String>)>> {
        Ok(paths
            .iter()
            .map(|p| (p.clone(), self.read_file(p)))
            .collect())
    }
}

/// Read-write access to doogat storage.
pub trait DoogatStore: DoogatSource {
    fn commit_file(&self, path: &str, content: &str, msg: &str) -> Result<CommitHash>;
    fn commit_files(&self, files: &[(&str, &str)], msg: &str) -> Result<CommitHash>;
    fn delete_file(&self, path: &str, msg: &str) -> Result<CommitHash>;
    fn delete_files(&self, paths: &[&str], msg: &str) -> Result<CommitHash>;
    fn commit_batch(
        &self,
        writes: &[(&str, &str)],
        deletes: &[&str],
        msg: &str,
    ) -> Result<CommitHash>;
}

/// Query and mutation operations on the doogat index.
pub trait DoogatIndex {
    fn index_doogat(&self, doogat: &ParsedDoogat) -> Result<()>;
    fn remove_doogat(&self, id: &str) -> Result<()>;
    fn search(&self, query: &str) -> Result<Vec<SearchResult>>;
    fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult>;
    fn resolve_path(&self, id: &str) -> Result<String>;
    fn query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>>;
    fn find_typedef_path(&self, type_name: &str) -> Result<Option<String>>;
    fn execute_sql(&self, sql: &str, params: &[&str]) -> Result<usize>;
}

/// Extended index operations for the SQL engine layer.
///
/// Separates sql_engine from the concrete `Index` type. Combines
/// `DoogatIndex` query/mutation methods with SQLite connection access
/// and materialization helpers that sql_engine needs for DDL, DML,
/// and transaction management.
pub trait SqlBackend: DoogatIndex {
    /// Raw SQLite connection for DDL execution, prepared statements, and
    /// transaction savepoints.
    ///
    /// **TRANSITIONAL EXCEPTION** — this method exposes a concrete `rusqlite::Connection`
    /// on the `SqlBackend` trait, violating the adapter-neutral boundary established by
    /// PRD 00141. Removing it requires rewriting dozens of call sites across
    /// `sql_engine/{dml,ddl,transaction,junction,mod}.rs`. That rewrite is scoped to a
    /// follow-up Phase 3 effort; until then this method is retained as a documented
    /// exception and must not be extended to new callers.
    fn sql_conn(&self) -> &Connection;

    /// Execute a SQL query returning column names and rows.
    fn query_raw_with_columns(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)>;

    /// Rebuild a single type's materialized SQLite table from index + source data.
    fn rematerialize_type(&self, type_name: &str, source: &dyn DoogatSource) -> Result<()>;

    /// Upsert a single doogat's row in its materialized type table.
    fn materialize_single(
        &self,
        schema: &TableSchema,
        id: &str,
        parsed: &ParsedDoogat,
    ) -> Result<()>;

    /// Insert junction table rows for REFERENCES columns for one doogat.
    fn populate_junction_tables(
        &self,
        schema: &TableSchema,
        id: &str,
        parsed: &ParsedDoogat,
    ) -> Result<()>;

    /// Replace junction table rows for changed REFERENCES columns for one doogat.
    ///
    /// `changed_cols` is a borrowed slice of column names because the typical
    /// UPDATE touches 1-3 columns; a `BTreeSet` allocation is wasteful at that
    /// size. PRD 00134 cycle-1 review C1 task #8.
    fn sync_junction_tables_for_columns(
        &self,
        schema: &TableSchema,
        id: &str,
        parsed: &ParsedDoogat,
        changed_cols: &[&str],
    ) -> Result<()>;

    /// Check whether a type uses folder-based storage.
    fn type_uses_folder(&self, type_name: &str, source: &dyn DoogatSource) -> bool;

    /// Find all doogats that link to the given target (by ID or path).
    fn backlinks_by_target(
        &self,
        target_id: &str,
        target_path: &str,
    ) -> Result<Vec<(String, String)>>;

    /// Enforce RESTRICT semantics for `NOT NULL REFERENCES` columns before a
    /// parent doogat is deleted. Returns `Err(Validation(..))` if any typed
    /// table holds `deleted_id` in a required FK column; the caller must not
    /// proceed with the delete. See `Index::check_restrict_blocks_delete`.
    fn check_restrict_blocks_delete(
        &self,
        source: &dyn DoogatSource,
        deleted_id: &str,
    ) -> Result<()>;
}

// EventPort is intentionally NOT defined here. The core service publishes no
// events; the `EventBus` lives only in `ddb-server` (see PRD 00142 inventory,
// Section C). Event dispatch is a transport concern owned by the server actor
// layer, so the core port set deliberately omits an event port.

/// Service-facing index port: the full set of index operations the
/// `DoogatService` layer calls through `self.index.*`.
///
/// Supertrait of `DoogatIndex` + `SqlBackend`, so it inherits the CRUD/search
/// methods (`index_doogat`, `remove_doogat`, `search`, `search_paginated`,
/// `resolve_path`, `query_raw`, `find_typedef_path`, `execute_sql`) and the SQL
/// engine helpers (`sql_conn`, `query_raw_with_columns`, `rematerialize_type`,
/// `materialize_single`, `populate_junction_tables`,
/// `sync_junction_tables_for_columns`, `type_uses_folder`, `backlinks_by_target`,
/// `check_restrict_blocks_delete`). This trait declares only the GAP methods not
/// already on those supertraits. Signatures are byte-identical to the inherent
/// methods on `Index` so a later task can swap the concrete `Index` field for a
/// generic `I: IndexPort` without touching any `self.index.method(...)` call site.
///
/// The former direct `self.index.conn` field access (11 sites in `service/crud.rs`
/// that pass `&self.index.conn` to `with_immediate_transaction` /
/// `prepare_typed_insert_validate` or call `.execute` directly) is served by the
/// inherited `SqlBackend::sql_conn() -> &Connection`. No separate connection
/// accessor is added here: `sql_conn` already returns exactly the raw
/// `&rusqlite::Connection` those sites need, and re-declaring it would duplicate a
/// supertrait method.
pub trait IndexPort: DoogatIndex + SqlBackend {
    /// Rebuild the index if the stored HEAD is stale (used by `ensure_fresh`).
    fn rebuild_if_stale(
        &self,
        repo: &impl DoogatSource,
    ) -> Result<Option<crate::types::RebuildReport>>;

    /// Full rebuild of the index from the repository.
    fn rebuild(&self, repo: &impl DoogatSource) -> Result<crate::types::RebuildReport>;

    /// Whether the index is stale relative to the repository HEAD.
    fn is_stale(&self, repo: &impl DoogatSource) -> Result<bool>;

    /// Store the current repository HEAD into the index meta table.
    fn store_head(&self, head: &str) -> Result<()>;

    /// Look up the indexed `updated_at` timestamp for one doogat ID.
    fn lookup_updated_at(&self, id: &str) -> Result<Option<String>>;

    /// Batch variant of `lookup_updated_at`.
    fn lookup_updated_at_batch(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>>;

    /// Execute a SQL query with adapter-neutral `QueryValue` parameters.
    fn query_raw_with_query_values(
        &self,
        sql: &str,
        params: &[crate::types::QueryValue],
    ) -> Result<Vec<Vec<String>>>;

    /// Load all typedef schemas keyed by table name.
    fn load_all_typedefs(
        &self,
        repo: &dyn DoogatSource,
    ) -> std::collections::HashMap<String, TableSchema>;

    /// Collect child doogats affected by a cascade delete of `deleted_id`.
    fn collect_cascade_children(
        &self,
        repo: &dyn DoogatSource,
        deleted_id: &str,
    ) -> Result<Vec<(String, String)>>;

    /// Remove junction rows referencing `deleted_id` as a target.
    fn cascade_junction_cleanup(
        &self,
        repo: &dyn DoogatSource,
        target_type: &str,
        deleted_id: &str,
    ) -> Result<()>;

    /// List all tags with their counts.
    fn list_tags(&self) -> Result<Vec<(String, i64)>>;

    /// Query tags with a filter.
    fn query_tags(
        &self,
        filter: &crate::types::TagQueryFilter,
    ) -> Result<Vec<crate::types::TagEntry>>;

    /// Find unlinked mentions of a target doogat.
    fn unlinked_mentions(&self, target_id: &str) -> Result<Vec<crate::types::UnlinkedMention>>;

    /// Suggest links for a source doogat.
    fn suggest_links(&self, source_id: &str, limit: usize)
        -> Result<Vec<crate::types::Suggestion>>;

    /// Find stale doogats per typedef staleness thresholds.
    fn stale_doogats(
        &self,
        repo: &(impl DoogatSource + GitHistory),
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::StaleDoogat>>;

    /// Find orphan doogats (no inbound links).
    fn orphan_doogats(&self, type_filter: Option<&str>) -> Result<Vec<crate::types::OrphanDoogat>>;

    /// Find recently updated doogats within `days`.
    fn recent_doogats(
        &self,
        days: u32,
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::RecentDoogat>>;

    /// Compute per-type link density.
    fn link_density(
        &self,
        type_filter: Option<&str>,
    ) -> Result<Vec<crate::types::LinkDensityEntry>>;

    /// Build a sequence tree rooted at `id`, bounded by `max_depth`.
    fn sequence_tree(
        &self,
        id: &str,
        max_depth: usize,
    ) -> Result<Vec<(crate::types::SequenceNode, usize)>>;

    /// Build a breadcrumb trail for a sequence node.
    fn sequence_breadcrumb(&self, id: &str) -> Result<Vec<crate::types::SequenceNode>>;

    /// Find broken sequence links.
    fn broken_sequences(&self) -> Result<Vec<crate::types::BrokenSequence>>;

    /// Summarize a sequence node's position.
    fn sequence_info(&self, id: &str) -> Result<crate::types::SequenceInfo>;

    /// List the direct children of a sequence node.
    fn sequence_children(&self, id: &str) -> Result<Vec<crate::types::SequenceNode>>;

    /// List paths of doogats linking to a target.
    fn backlinks(&self, target_path: &str) -> Result<Vec<String>>;

    /// List `(id, path)` of doogats linking to a target.
    fn backlinking_doogat_paths(&self, target: &str) -> Result<Vec<(String, String)>>;

    /// Find resurrected doogats (links to previously-deleted targets now present).
    fn resurrected_doogats(&self) -> Result<Vec<(String, String)>>;

    /// Find broken backlinks (links to targets that no longer exist).
    fn broken_backlinks(&self) -> Result<Vec<(String, String)>>;

    /// Infer a type's table schema from indexed data and the repository.
    fn infer_schema(
        &self,
        type_name: &str,
        repo: &(impl DoogatSource + ?Sized),
    ) -> Result<TableSchema>;

    /// Execute a SQL query with positional `rusqlite::Value` parameters.
    fn query_raw_with_params(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<Vec<String>>>;

    /// Paginated search with structured filters (type/tag/field negation).
    fn search_paginated_filtered(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        filters: &crate::types::SearchFilters,
    ) -> Result<PaginatedSearchResult>;
}

/// Materialized-typedef operations for typed writes and SQL-facing behavior.
///
/// Signatures are byte-identical to the inherent `Index` methods so the later
/// concrete-to-generic swap needs no call-site edits. `materialize_single` and
/// `type_uses_folder` are intentionally NOT re-declared here — they already live
/// on the `SqlBackend` supertrait and are reached through `IndexPort`.
pub trait TypedMaterializationPort {
    /// Infer a type's table schema from indexed data and the repository.
    fn infer_schema(
        &self,
        type_name: &str,
        repo: &(impl DoogatSource + ?Sized),
    ) -> Result<TableSchema>;

    /// Load all typedef schemas keyed by table name.
    fn load_all_typedefs(
        &self,
        repo: &dyn DoogatSource,
    ) -> std::collections::HashMap<String, TableSchema>;
}

/// Optional secondary NoSQL mirror (best-effort dual write).
///
/// Production is implemented by the Redb index; tests by a no-op. Methods return
/// a fallible `Result<()>` rather than collapsing to a silent `()` so a caller
/// can surface a visible warning when a mirror write fails. Covers exactly the
/// dual-write operations in `service/crud.rs` (`nosql_index_doogat`,
/// `nosql_remove_doogat`): mirror an upserted doogat, remove a mirrored doogat.
pub trait NoSqlMirrorPort {
    /// Mirror an upserted doogat into the secondary NoSQL index.
    fn mirror_index_doogat(&self, doogat: &ParsedDoogat) -> Result<()>;

    /// Remove a mirrored doogat from the secondary NoSQL index.
    fn mirror_remove_doogat(&self, id: &str) -> Result<()>;
}

/// No-op mirror used when the secondary NoSQL index is unavailable or unwanted.
pub struct NoopMirror;

impl NoSqlMirrorPort for NoopMirror {
    fn mirror_index_doogat(&self, _doogat: &ParsedDoogat) -> Result<()> {
        Ok(())
    }

    fn mirror_remove_doogat(&self, _id: &str) -> Result<()> {
        Ok(())
    }
}

/// CRDT-based conflict resolution strategy.
pub trait ConflictResolver {
    fn resolve_conflicts(
        &self,
        conflicts: Vec<ConflictFile>,
        strategy: Option<&str>,
    ) -> Result<Vec<ResolvedFile>>;
}

/// Remote repository operations (add, fetch, push).
pub trait GitRemote {
    /// Register a named remote.
    fn add_remote(&self, name: &str, url: &str) -> Result<()>;

    /// Fetch from a remote branch.
    fn fetch(&self, remote: &str, branch: &str) -> Result<()>;

    /// Push to a remote branch.
    fn push(&self, remote: &str, branch: &str) -> Result<()>;

    /// Delete a remote-tracking ref, if it exists.
    fn delete_remote_ref(&self, remote: &str, branch: &str) -> Result<()>;
}

/// Merge operations (merge remote branches, create merge commits).
pub trait GitMerge {
    /// Merge a fetched remote branch, returning the merge result.
    fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult>;

    /// Create a merge commit with resolved files and two parents.
    fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary: &[(&str, &str)],
        losers: &[crate::types::CollisionLoser],
        message: &str,
        theirs: &CommitHash,
    ) -> Result<CommitHash>;
}

/// Commit introspection, tree walking, and history queries.
pub trait GitHistory {
    /// Compute merge-base of two commit OIDs (as hex strings).
    fn merge_base(&self, a: &str, b: &str) -> Result<String>;

    /// Return the number of parents of the given commit.
    fn commit_parent_count(&self, commit_oid: &str) -> Result<usize>;

    /// Return the OID (hex string) of the nth parent of a commit.
    fn commit_parent_oid(&self, commit_oid: &str, n: usize) -> Result<String>;

    /// Read a file's text content from a specific commit's tree.
    fn read_file_at(&self, commit_oid: &str, rel_path: &str) -> Result<String>;

    /// Walk a commit's tree under `prefix`, returning `(path, text_content)` for
    /// each blob that can be decoded as UTF-8. Non-UTF-8 blobs are silently skipped.
    fn walk_tree_files(&self, commit_oid: &str, prefix: &str) -> Result<Vec<(String, String)>>;

    /// Find the HLC timestamp from the most recent commit that touched `path`,
    /// starting from the given commit OID.
    fn find_hlc_for_path(&self, commit_oid: &str, path: &str) -> Option<crate::hlc::Hlc>;

    /// Return the ISO 8601 date of the most recent commit that touched `rel_path`.
    fn revision_date(&self, rel_path: &str) -> Result<Option<String>>;
}

/// Binary file operations (commit binary blobs, read raw blobs).
pub trait GitBinary {
    /// Write binary content to a file, stage it, and commit.
    fn commit_binary_file(&self, rel_path: &str, bytes: &[u8], message: &str)
        -> Result<CommitHash>;

    /// Write a binary file and one or more text files in a single atomic commit.
    fn commit_binary_and_text(
        &self,
        binary_path: &str,
        bytes: &[u8],
        text_files: &[(&str, &str)],
        message: &str,
    ) -> Result<CommitHash>;

    /// Read raw blob bytes by OID string.
    fn read_blob(&self, oid: &str) -> Result<Vec<u8>>;
}

/// File rename operations.
pub trait GitRename {
    /// Rename (move) a file in git.
    fn rename_file(&self, old_path: &str, new_path: &str, message: &str) -> Result<CommitHash>;
}

/// Desktop-only hooks with default no-op implementations (commit-graph,
/// session counters). Backends that support these override the defaults.
pub trait GitDesktopHooks {
    /// Suppress per-commit commit-graph writes (for batch operations).
    fn set_skip_commit_graph(&self, _skip: bool) {}

    /// Write the commit-graph file for faster traversal.
    fn write_commit_graph(&self) {}

    /// Increment session commit counter, return new value.
    fn increment_session_commits(&self) -> u32 {
        0
    }

    /// Reset session commit counter to zero.
    fn reset_session_commits(&self) {}
}

/// Unified git storage backend trait. Composes all focused sub-traits
/// into a single bound for code that needs the full git backend.
///
/// Allows swapping libgit2 for gitoxide (or other backends) per-feature
/// without changing callers.
pub trait GitBackend:
    DoogatSource
    + DoogatStore
    + GitRemote
    + GitMerge
    + GitHistory
    + GitBinary
    + GitRename
    + GitDesktopHooks
{
    /// Repository root path on the filesystem.
    fn repo_path(&self) -> &Path;

    /// Load repository config from `.ddb.toml`.
    fn load_config(&self) -> Result<RepoConfig>;
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;

    /// In-memory mock implementing DoogatSource for unit tests.
    pub struct MockSource {
        pub files: HashMap<String, String>,
        pub head: String,
    }

    impl Default for MockSource {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockSource {
        pub fn new() -> Self {
            Self {
                files: HashMap::new(),
                head: "abc123".to_string(),
            }
        }
    }

    impl DoogatSource for MockSource {
        fn list_doogats(&self) -> Result<Vec<String>> {
            let mut paths: Vec<String> = self
                .files
                .keys()
                .filter(|p| p.starts_with("ddb/") && p.ends_with(".md"))
                .cloned()
                .collect();
            paths.sort();
            Ok(paths)
        }

        fn read_file(&self, path: &str) -> Result<String> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| crate::error::DoogatError::NotFound(path.to_string()))
        }

        fn head_oid(&self) -> Result<CommitHash> {
            Ok(CommitHash(self.head.clone()))
        }

        fn diff_paths(&self, _old_oid: &str, _new_oid: &str) -> Result<Vec<(DiffKind, String)>> {
            // Mock always returns empty diff — tests that need diffs use GitRepo directly
            Ok(Vec::new())
        }
    }
}
