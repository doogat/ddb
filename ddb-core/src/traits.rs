use crate::error::Result;
use crate::types::{
    CommitHash, ConflictFile, DiffKind, PaginatedSearchResult, ParsedDoogat, ResolvedFile,
    SearchResult,
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

/// CRDT-based conflict resolution strategy.
pub trait ConflictResolver {
    fn resolve_conflicts(
        &self,
        conflicts: Vec<ConflictFile>,
        strategy: Option<&str>,
    ) -> Result<Vec<ResolvedFile>>;
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
