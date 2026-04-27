use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

use super::value::Value;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct DoogatId(pub String);

impl fmt::Display for DoogatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for DoogatId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DoogatIdVisitor;

        impl<'de> serde::de::Visitor<'de> for DoogatIdVisitor {
            type Value = DoogatId;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string or integer doogat ID")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<DoogatId, E> {
                Ok(DoogatId(v.to_string()))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<DoogatId, E> {
                Ok(DoogatId(v.to_string()))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<DoogatId, E> {
                Ok(DoogatId(v.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(
                self,
                v: String,
            ) -> std::result::Result<DoogatId, E> {
                Ok(DoogatId(v))
            }
        }

        deserializer.deserialize_any(DoogatIdVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Zone {
    Frontmatter,
    Body,
    Reference,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DoogatMeta {
    pub id: Option<DoogatId>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub doogat_type: Option<String>,
    pub tags: Vec<String>,
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InlineField {
    pub key: String,
    pub value: String,
    pub zone: Zone,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LinkKind {
    WikiLink,
    MarkdownLink,
    Embed,
    BareUrl,
}

impl LinkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkKind::WikiLink => "wikilink",
            LinkKind::MarkdownLink => "markdown",
            LinkKind::Embed => "embed",
            LinkKind::BareUrl => "url",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Link {
    pub target: String,
    pub display: Option<String>,
    pub section: Option<String>,
    pub kind: LinkKind,
    pub zone: Zone,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Section {
    pub heading: String,
    pub level: u8,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CheckboxState {
    Open,
    Done,
    Info,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckboxItem {
    pub state: CheckboxState,
    pub content: String,
    pub date: Option<String>,
    pub due_date: Option<String>,
    pub line_number: usize,
    pub indent_level: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParsedDoogat {
    pub meta: DoogatMeta,
    pub body: String,
    #[serde(default)]
    pub sections: Vec<Section>,
    pub reference_section: String,
    pub inline_fields: Vec<InlineField>,
    pub links: Vec<Link>,
    #[serde(default)]
    pub body_tags: Vec<String>,
    #[serde(default)]
    pub checkboxes: Vec<CheckboxItem>,
    pub path: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Doogat {
    pub raw_frontmatter: String,
    pub body: String,
    pub reference_section: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    #[default]
    Active,
    Stale,
    Retired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub known_heads: Vec<String>,
    pub last_sync: Option<String>,
    /// Last HLC timestamp (persisted for clock continuity across restarts).
    #[serde(default)]
    pub hlc: Option<String>,
    /// Node lifecycle status.
    #[serde(default)]
    pub status: NodeStatus,
    /// ISO 8601 timestamp when this node was first registered.
    #[serde(default)]
    pub created: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SyncState {
    pub known_heads: Vec<String>,
    pub last_sync: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub direction: String,
    pub commits_transferred: usize,
    pub conflicts_resolved: usize,
    pub resurrected: usize,
    pub collisions_reassigned: usize,
}

#[derive(Debug, Clone)]
pub struct ConflictFile {
    pub path: String,
    pub ancestor: Option<String>,
    pub ours: String,
    pub theirs: String,
    /// HLC from the commit that produced "ours" content.
    pub ours_hlc: Option<crate::hlc::Hlc>,
    /// HLC from the commit that produced "theirs" content.
    pub theirs_hlc: Option<crate::hlc::Hlc>,
    /// Raw blob OID for "ours" side (for binary conflict resolution).
    pub ours_blob_oid: Option<String>,
    /// Raw blob OID for "theirs" side (for binary conflict resolution).
    pub theirs_blob_oid: Option<String>,
}

/// Domain-level commit identifier, decoupled from git2::Oid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHash(pub String);

impl fmt::Display for CommitHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug)]
pub enum MergeResult {
    AlreadyUpToDate,
    FastForward(CommitHash),
    Clean(CommitHash),
    Conflicts(Vec<ConflictFile>, CommitHash),
}

#[derive(Debug, Clone)]
pub struct ResolvedFile {
    pub path: String,
    pub content: String,
    pub fm_crdt_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub struct CompactOptions {
    pub force: bool,
    pub skip_backup: bool,
    pub backup_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone)]
pub struct CompactionReport {
    pub files_removed: usize,
    pub crdt_docs_compacted: usize,
    pub gc_success: bool,
    pub crdt_temp_bytes_before: u64,
    pub crdt_temp_bytes_after: u64,
    pub crdt_temp_files_before: usize,
    pub crdt_temp_files_after: usize,
    pub repo_bytes_before: u64,
    pub repo_bytes_after: u64,
    pub backup_path: Option<std::path::PathBuf>,
}

/// Kind of change detected by diff_tree_to_tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BundleManifest {
    pub source_node: String,
    pub target_node: String,
    pub timestamp: String,
    pub format_version: u32,
}

#[derive(Debug, Clone)]
pub struct MaintenanceReport {
    pub tasks_run: Vec<String>,
    pub success: bool,
    pub duration_ms: u64,
    pub fallback_used: bool,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub rank: f64,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub fields: Option<BTreeMap<String, String>>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PaginatedSearchResult {
    pub hits: Vec<SearchResult>,
    pub total_count: usize,
}

#[derive(Debug, Clone)]
pub enum SearchFieldOp {
    Eq(String),
    Contains(String),
    In(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct SearchFieldFilter {
    pub field: String,
    pub op: SearchFieldOp,
}

#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    pub types: Option<Vec<String>>,
    pub tag: Option<String>,
    pub where_filters: Option<Vec<SearchFieldFilter>>,
}

#[derive(Debug, Clone, Default)]
pub struct RenameReport {
    pub updated: Vec<String>,
    pub unresolvable: Vec<String>,
}

// ── Discovery types ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UnlinkedMention {
    pub source_id: String,
    pub source_title: String,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub id: String,
    pub title: String,
    pub score: f64,
    pub shared_tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateSource {
    GitRevision,
    FrontmatterDate,
    IndexerUpdatedAt,
}

impl fmt::Display for DateSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateSource::GitRevision => write!(f, "git"),
            DateSource::FrontmatterDate => write!(f, "frontmatter"),
            DateSource::IndexerUpdatedAt => write!(f, "indexer"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StaleDoogat {
    pub id: String,
    pub title: String,
    pub doogat_type: String,
    pub last_updated: String,
    pub date_source: DateSource,
    pub days_stale: u32,
    pub threshold_days: u32,
}

#[derive(Debug, Clone)]
pub struct OrphanDoogat {
    pub id: String,
    pub title: String,
    pub doogat_type: String,
    pub outgoing_links: usize,
}

#[derive(Debug, Clone)]
pub struct RecentDoogat {
    pub id: String,
    pub title: String,
    pub doogat_type: String,
    pub last_modified: String,
}

#[derive(Debug, Clone)]
pub struct LinkDensityEntry {
    pub id: String,
    pub title: String,
    pub doogat_type: String,
    pub inbound_links: usize,
    pub outbound_links: usize,
    pub density_score: usize,
}

// ── Sequence types ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SequenceNode {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct SequenceInfo {
    pub parent: Option<SequenceNode>,
    pub children: Vec<SequenceNode>,
    pub breadcrumb: Vec<SequenceNode>,
}

#[derive(Debug, Clone)]
pub struct BrokenSequence {
    pub doogat_id: String,
    pub broken_parent_id: String,
}

/// Dry-run compaction info returned by `DoogatService::compact_dry_run`.
#[derive(Debug, Clone)]
pub struct CompactDryRunInfo {
    pub shared_head: Option<String>,
    pub crdt_temp_files: usize,
    pub default_backup_path: std::path::PathBuf,
}

/// A single tag-doogat association from the `_ddb_tags` table.
#[derive(Debug, Clone)]
pub struct TagEntry {
    pub doogat_id: String,
    pub tag: String,
    pub source: String,
}

/// Filters for querying individual tag entries.
#[derive(Debug, Clone, Default)]
pub struct TagQueryFilter {
    pub doogat_id_eq: Option<String>,
    pub doogat_id_in: Option<Vec<String>>,
    pub tag_eq: Option<String>,
    pub tag_contains: Option<String>,
    pub tag_in: Option<Vec<String>>,
}

/// Filter parameters for querying doogats by type, tag, backlinks, and fields.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub doogat_type: Option<String>,
    pub tag: Option<String>,
    pub backlinks_of: Option<String>,
    pub field_filters: Vec<(String, String)>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub sort_field: Option<String>,
    pub sort_desc: Option<bool>,
}

/// Action to take when an INSERT violates a unique constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictAction {
    /// Fail with an error if a duplicate unique constraint is violated (default).
    #[default]
    Error,
    /// Skip the insert and return the existing doogat when a unique constraint matches.
    Ignore,
}

/// Input for batch creation of doogats.
///
/// `title` is optional: when `None`, the engine renders the title from the
/// typedef's `title_template` if one is declared; otherwise the create is
/// rejected with `NOT_NULL_VIOLATION` on the title column.
#[derive(Debug, Clone)]
pub struct BatchCreateInput {
    pub title: Option<String>,
    pub body: Option<String>,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub fields: std::collections::BTreeMap<String, Value>,
    pub on_conflict: ConflictAction,
}

/// Input for a single item in a batch update operation.
#[derive(Debug, Clone)]
pub struct BatchUpdateInput {
    pub id: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub tags: Option<Vec<String>>,
    pub doogat_type: Option<String>,
    pub fields: Option<std::collections::BTreeMap<String, Value>>,
    pub unset_fields: Option<Vec<String>>,
}

/// Query parameters for typed (materialized) table queries.
#[derive(Debug, Clone)]
pub struct TypedListQuery {
    pub table_name: String,
    pub where_sql: String,
    pub params: Vec<rusqlite::types::Value>,
    pub order_sql: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// When set, deduplicate results by this column (GROUP BY).
    pub distinct: Option<String>,
}

/// Metadata for a file attached to a doogat, stored in `reference/{doogat_id}/`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AttachmentInfo {
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Relative path from repo root, e.g. `reference/20260301130000/photo.jpg`
    pub path: String,
}

impl AttachmentInfo {
    /// Detect MIME type from a filename's extension.
    pub fn mime_from_filename(filename: &str) -> &'static str {
        let ext = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "ico" => "image/x-icon",
            "pdf" => "application/pdf",
            "json" => "application/json",
            "xml" => "application/xml",
            "zip" => "application/zip",
            "gz" | "gzip" => "application/gzip",
            "tar" => "application/x-tar",
            "txt" => "text/plain",
            "md" => "text/markdown",
            "html" | "htm" => "text/html",
            "css" => "text/css",
            "csv" => "text/csv",
            "js" => "text/javascript",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            _ => "application/octet-stream",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_from_filename_common_types() {
        assert_eq!(
            AttachmentInfo::mime_from_filename("photo.jpg"),
            "image/jpeg"
        );
        assert_eq!(
            AttachmentInfo::mime_from_filename("photo.JPEG"),
            "image/jpeg"
        );
        assert_eq!(AttachmentInfo::mime_from_filename("icon.png"), "image/png");
        assert_eq!(
            AttachmentInfo::mime_from_filename("doc.pdf"),
            "application/pdf"
        );
        assert_eq!(AttachmentInfo::mime_from_filename("data.csv"), "text/csv");
        assert_eq!(AttachmentInfo::mime_from_filename("page.html"), "text/html");
        assert_eq!(
            AttachmentInfo::mime_from_filename("notes.md"),
            "text/markdown"
        );
    }

    #[test]
    fn mime_from_filename_fallback() {
        assert_eq!(
            AttachmentInfo::mime_from_filename("file.xyz"),
            "application/octet-stream"
        );
        assert_eq!(
            AttachmentInfo::mime_from_filename("noext"),
            "application/octet-stream"
        );
    }
}
