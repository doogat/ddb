//! Result types for the application contract layer.
//!
//! These DTOs are the canonical shapes that PRD 00148's cross-interface
//! conformance harness will assert against once it lands. They have no
//! in-tree consumer yet by design — adding the shapes first lets the
//! harness be written against a stable contract.

use crate::types::ParsedDoogat;

/// A backlink that is broken after a delete because its source still references the removed doogat.
#[derive(Debug, Clone)]
pub struct BrokenBacklink {
    pub source_id: String,
    pub source_path: String,
}

/// Returned by a successful delete operation.
#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub broken_backlinks: Vec<BrokenBacklink>,
}

/// Returned by a successful create operation.
#[derive(Debug, Clone)]
pub struct CreateResult {
    pub doogat: ParsedDoogat,
}

/// Returned by a successful read (get-by-id) operation.
#[derive(Debug, Clone)]
pub struct ReadResult {
    pub doogat: ParsedDoogat,
}

/// Returned by a successful update operation.
#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub doogat: ParsedDoogat,
}

/// Returned by a list (or paginated list) operation.
///
/// `total` is the unfiltered count for the filter scope; defaults to `items.len()` when
/// pagination is not applied.
#[derive(Debug, Clone)]
pub struct ListResult {
    pub items: Vec<ParsedDoogat>,
    pub total: usize,
}

/// Returned by a search operation; carries the query string for round-trip diagnostics.
///
/// Named `AppSearchResult` (not `SearchResult`) because `crate::types::SearchResult`
/// is the long-standing indexer-level type and the bare name is already taken in
/// `ddb_core`. The `App` prefix mirrors `AppOutput`/`AppError`/`AppWarning`.
#[derive(Debug, Clone)]
pub struct AppSearchResult {
    pub items: Vec<ParsedDoogat>,
    pub total: usize,
    pub query: String,
}
