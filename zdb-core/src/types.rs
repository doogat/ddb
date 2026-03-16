use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Repository-level configuration stored in `.zetteldb.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoConfig {
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub crdt: CrdtConfig,
    #[serde(default)]
    pub maintenance: MaintenanceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Days before a non-syncing node is considered stale.
    #[serde(default = "default_stale_ttl_days")]
    pub stale_ttl_days: u32,
    /// CRDT temp cleanup threshold in MB.
    #[serde(default = "default_threshold_mb")]
    pub threshold_mb: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            stale_ttl_days: default_stale_ttl_days(),
            threshold_mb: default_threshold_mb(),
        }
    }
}

fn default_stale_ttl_days() -> u32 {
    90
}
fn default_threshold_mb() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrdtConfig {
    /// Fallback CRDT strategy when typedef doesn't specify one.
    #[serde(default = "default_crdt_strategy")]
    pub default_strategy: String,
}

impl Default for CrdtConfig {
    fn default() -> Self {
        Self {
            default_strategy: default_crdt_strategy(),
        }
    }
}

fn default_crdt_strategy() -> String {
    "preset:default".to_string()
}

fn default_write_threshold() -> u32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceConfig {
    #[serde(default)]
    pub auto_enabled: bool,
    #[serde(default = "default_write_threshold")]
    pub write_threshold: u32,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            auto_enabled: false,
            write_threshold: default_write_threshold(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MaintenanceReport {
    pub tasks_run: Vec<String>,
    pub success: bool,
    pub duration_ms: u64,
    pub fallback_used: bool,
}

/// Domain-level value type, decoupled from serde_yaml::Value.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_sequence(&self) -> Option<&[Value]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_mapping(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_sequence(&self) -> bool {
        matches!(self, Value::List(_))
    }

    pub fn is_mapping(&self) -> bool {
        matches!(self, Value::Map(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ZettelId(pub String);

impl fmt::Display for ZettelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for ZettelId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ZettelIdVisitor;

        impl<'de> serde::de::Visitor<'de> for ZettelIdVisitor {
            type Value = ZettelId;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string or integer zettel ID")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<ZettelId, E> {
                Ok(ZettelId(v.to_string()))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<ZettelId, E> {
                Ok(ZettelId(v.to_string()))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<ZettelId, E> {
                Ok(ZettelId(v.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(
                self,
                v: String,
            ) -> std::result::Result<ZettelId, E> {
                Ok(ZettelId(v))
            }
        }

        deserializer.deserialize_any(ZettelIdVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Zone {
    Frontmatter,
    Body,
    Reference,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ZettelMeta {
    pub id: Option<ZettelId>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub zettel_type: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Link {
    pub target: String,
    pub display: Option<String>,
    pub section: Option<String>,
    pub kind: LinkKind,
    pub zone: Zone,
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
pub struct ParsedZettel {
    pub meta: ZettelMeta,
    pub body: String,
    pub reference_section: String,
    pub inline_fields: Vec<InlineField>,
    pub links: Vec<Link>,
    #[serde(default)]
    pub body_tags: Vec<String>,
    #[serde(default)]
    pub checkboxes: Vec<CheckboxItem>,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Zettel {
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
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub references: Option<String>,
    pub zone: Option<Zone>,
    pub required: bool,
    pub search_boost: Option<f64>,
    pub allowed_values: Option<Vec<String>>,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
    pub crdt_strategy: Option<String>,
    pub template_sections: Vec<String>,
    pub folder: bool,
}

#[derive(Debug, Clone)]
pub enum ConsistencyWarning {
    MalformedYaml {
        path: String,
        error: String,
    },
    CrossZoneDuplicate {
        path: String,
        key: String,
    },
    MissingRequired {
        path: String,
        type_name: String,
        field: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct RebuildReport {
    pub indexed: usize,
    pub tables_materialized: usize,
    pub types_inferred: Vec<String>,
    pub warnings: Vec<ConsistencyWarning>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub rank: f64,
}

#[derive(Debug, Clone)]
pub struct PaginatedSearchResult {
    pub hits: Vec<SearchResult>,
    pub total_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct RenameReport {
    pub updated: Vec<String>,
    pub unresolvable: Vec<String>,
}

/// Metadata for a file attached to a zettel, stored in `reference/{zettel_id}/`.
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
