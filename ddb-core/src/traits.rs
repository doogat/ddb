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
}

/// Merge operations (merge remote branches, create merge commits).
pub trait GitMerge {
    /// Merge a fetched remote branch, returning the merge result.
    fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult>;

    /// Create a merge commit with resolved files and two parents.
    fn commit_merge(
        &self,
        files: &[(&str, &str)],
        binary_paths: &[&str],
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
    fn walk_tree_files(
        &self,
        commit_oid: &str,
        prefix: &str,
    ) -> Result<Vec<(String, String)>>;

    /// Find the HLC timestamp from the most recent commit that touched `path`,
    /// starting from the given commit OID.
    fn find_hlc_for_path(
        &self,
        commit_oid: &str,
        path: &str,
    ) -> Option<crate::hlc::Hlc>;

    /// Return the ISO 8601 date of the most recent commit that touched `rel_path`.
    fn revision_date(&self, rel_path: &str) -> Result<Option<String>>;
}

/// Binary file operations (commit binary blobs, read raw blobs).
pub trait GitBinary {
    /// Write binary content to a file, stage it, and commit.
    fn commit_binary_file(
        &self,
        rel_path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<CommitHash>;

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
    fn rename_file(
        &self,
        old_path: &str,
        new_path: &str,
        message: &str,
    ) -> Result<CommitHash>;
}

/// Desktop-only hooks with default no-op implementations (commit-graph,
/// session counters). Backends that support these override the defaults.
pub trait GitDesktopHooks {
    /// Suppress per-commit commit-graph writes (for batch operations).
    fn set_skip_commit_graph(&self, _skip: bool) {}

    /// Write the commit-graph file for faster traversal.
    fn write_commit_graph(&self) {}

    /// Increment session commit counter, return new value.
    fn increment_session_commits(&self) -> u32 { 0 }

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
