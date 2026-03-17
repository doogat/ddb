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

// ── Path navigation ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    KeyNotFound {
        path: String,
        segment: String,
    },
    IndexOutOfBounds {
        path: String,
        index: usize,
        length: usize,
    },
    TypeMismatch {
        path: String,
        expected: &'static str,
        actual: &'static str,
    },
    InvalidPath {
        path: String,
        reason: String,
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::KeyNotFound { path, segment } => {
                write!(f, "key not found: \"{segment}\" in path \"{path}\"")
            }
            PathError::IndexOutOfBounds {
                path,
                index,
                length,
            } => {
                write!(
                    f,
                    "index {index} out of bounds (length {length}) in path \"{path}\""
                )
            }
            PathError::TypeMismatch {
                path,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "type mismatch at \"{path}\": expected {expected}, got {actual}"
                )
            }
            PathError::InvalidPath { path, reason } => {
                write!(f, "invalid path \"{path}\": {reason}")
            }
        }
    }
}

impl std::error::Error for PathError {}

/// Parse a dot/bracket notation path into segments.
///
/// - `.` separates map keys
/// - `[N]` indexes into lists (0-based)
/// - `\.` is a literal dot within a key name
/// - Empty segments are rejected
pub fn parse_path(path: &str) -> std::result::Result<Vec<PathSegment>, PathError> {
    if path.is_empty() {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "empty path".to_string(),
        });
    }

    let mut segments = Vec::new();
    let mut current_key = String::new();
    let mut chars = path.chars().peekable();
    let mut last_was_dot = false;

    while let Some(ch) = chars.next() {
        last_was_dot = false;
        match ch {
            '\\' => {
                // Escaped character — next char is literal
                match chars.next() {
                    Some(escaped) => current_key.push(escaped),
                    None => current_key.push('\\'),
                }
            }
            '.' => {
                last_was_dot = true;
                if current_key.is_empty() {
                    // Allow dot after bracket (e.g. "a[0].b") — just a separator
                    if !matches!(segments.last(), Some(PathSegment::Index(_))) {
                        return Err(PathError::InvalidPath {
                            path: path.to_string(),
                            reason: "empty segment".to_string(),
                        });
                    }
                } else {
                    segments.push(PathSegment::Key(std::mem::take(&mut current_key)));
                }
            }
            '[' => {
                // Flush any pending key
                if !current_key.is_empty() {
                    segments.push(PathSegment::Key(std::mem::take(&mut current_key)));
                }
                // Parse index number
                let mut index_str = String::new();
                loop {
                    match chars.next() {
                        Some(']') => break,
                        Some(d) if d.is_ascii_digit() => index_str.push(d),
                        Some(other) => {
                            return Err(PathError::InvalidPath {
                                path: path.to_string(),
                                reason: format!("unexpected '{other}' in index"),
                            });
                        }
                        None => {
                            return Err(PathError::InvalidPath {
                                path: path.to_string(),
                                reason: "unclosed bracket".to_string(),
                            });
                        }
                    }
                }
                if index_str.is_empty() {
                    return Err(PathError::InvalidPath {
                        path: path.to_string(),
                        reason: "empty index".to_string(),
                    });
                }
                let idx: usize = index_str.parse().map_err(|_| PathError::InvalidPath {
                    path: path.to_string(),
                    reason: format!("invalid index: {index_str}"),
                })?;
                segments.push(PathSegment::Index(idx));
            }
            _ => {
                current_key.push(ch);
            }
        }
    }

    // Reject trailing dot
    if last_was_dot {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "trailing dot".to_string(),
        });
    }

    // Flush trailing key
    if !current_key.is_empty() {
        segments.push(PathSegment::Key(current_key));
    }

    if segments.is_empty() {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "no segments".to_string(),
        });
    }

    Ok(segments)
}

/// Navigate a dot/bracket path starting from a `BTreeMap`, without wrapping in `Value::Map`.
/// Useful when you have `&BTreeMap<String, Value>` (e.g., `extra` fields) and want to avoid cloning.
pub fn get_path_in_map<'a>(
    map: &'a BTreeMap<String, Value>,
    path: &str,
) -> std::result::Result<&'a Value, PathError> {
    let segments = parse_path(path)?;
    if segments.is_empty() {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "no segments".to_string(),
        });
    }

    // First segment must be a Key into the map
    let first = &segments[0];
    let PathSegment::Key(key) = first else {
        return Err(PathError::TypeMismatch {
            path: path.to_string(),
            expected: "map",
            actual: "map (index on root)",
        });
    };
    let mut current = map.get(key).ok_or_else(|| PathError::KeyNotFound {
        path: path.to_string(),
        segment: key.clone(),
    })?;

    for seg in &segments[1..] {
        match seg {
            PathSegment::Key(k) => match current {
                Value::Map(m) => {
                    current = m.get(k).ok_or_else(|| PathError::KeyNotFound {
                        path: path.to_string(),
                        segment: k.clone(),
                    })?;
                }
                other => {
                    return Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "map",
                        actual: other.type_name(),
                    });
                }
            },
            PathSegment::Index(idx) => match current {
                Value::List(list) => {
                    let len = list.len();
                    current = list.get(*idx).ok_or_else(|| PathError::IndexOutOfBounds {
                        path: path.to_string(),
                        index: *idx,
                        length: len,
                    })?;
                }
                other => {
                    return Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "list",
                        actual: other.type_name(),
                    });
                }
            },
        }
    }
    Ok(current)
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "bool",
            Value::List(_) => "list",
            Value::Map(_) => "map",
        }
    }

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

    /// Navigate a nested `Value` tree using dot/bracket path notation.
    pub fn get_path(&self, path: &str) -> std::result::Result<&Value, PathError> {
        let segments = parse_path(path)?;
        let mut current = self;
        for seg in &segments {
            match seg {
                PathSegment::Key(key) => match current {
                    Value::Map(map) => {
                        current = map.get(key).ok_or_else(|| PathError::KeyNotFound {
                            path: path.to_string(),
                            segment: key.clone(),
                        })?;
                    }
                    other => {
                        return Err(PathError::TypeMismatch {
                            path: path.to_string(),
                            expected: "map",
                            actual: other.type_name(),
                        });
                    }
                },
                PathSegment::Index(idx) => match current {
                    Value::List(list) => {
                        current = list.get(*idx).ok_or_else(|| PathError::IndexOutOfBounds {
                            path: path.to_string(),
                            index: *idx,
                            length: list.len(),
                        })?;
                    }
                    other => {
                        return Err(PathError::TypeMismatch {
                            path: path.to_string(),
                            expected: "list",
                            actual: other.type_name(),
                        });
                    }
                },
            }
        }
        Ok(current)
    }

    /// Set a value at a dot/bracket path, creating intermediate containers as needed.
    pub fn set_path(&mut self, path: &str, value: Value) -> std::result::Result<(), PathError> {
        let segments = parse_path(path)?;
        let mut current = self;

        for (i, seg) in segments.iter().enumerate() {
            let is_last = i == segments.len() - 1;

            if is_last {
                match seg {
                    PathSegment::Key(key) => match current {
                        Value::Map(map) => {
                            map.insert(key.clone(), value);
                            return Ok(());
                        }
                        other => {
                            return Err(PathError::TypeMismatch {
                                path: path.to_string(),
                                expected: "map",
                                actual: other.type_name(),
                            });
                        }
                    },
                    PathSegment::Index(idx) => match current {
                        Value::List(list) => {
                            while list.len() <= *idx {
                                list.push(Value::String(String::new()));
                            }
                            list[*idx] = value;
                            return Ok(());
                        }
                        other => {
                            return Err(PathError::TypeMismatch {
                                path: path.to_string(),
                                expected: "list",
                                actual: other.type_name(),
                            });
                        }
                    },
                }
            }

            // Navigate intermediate, creating containers if needed
            let next_is_index = matches!(segments.get(i + 1), Some(PathSegment::Index(_)));
            match seg {
                PathSegment::Key(key) => match current {
                    Value::Map(map) => {
                        current = map.entry(key.clone()).or_insert_with(|| {
                            if next_is_index {
                                Value::List(Vec::new())
                            } else {
                                Value::Map(BTreeMap::new())
                            }
                        });
                    }
                    other => {
                        return Err(PathError::TypeMismatch {
                            path: path.to_string(),
                            expected: "map",
                            actual: other.type_name(),
                        });
                    }
                },
                PathSegment::Index(idx) => match current {
                    Value::List(list) => {
                        while list.len() <= *idx {
                            list.push(if next_is_index {
                                Value::List(Vec::new())
                            } else {
                                Value::Map(BTreeMap::new())
                            });
                        }
                        current = &mut list[*idx];
                    }
                    other => {
                        return Err(PathError::TypeMismatch {
                            path: path.to_string(),
                            expected: "list",
                            actual: other.type_name(),
                        });
                    }
                },
            }
        }

        Ok(())
    }

    /// Remove a value at a dot/bracket path, returning the removed value.
    pub fn remove_path(&mut self, path: &str) -> std::result::Result<Value, PathError> {
        let segments = parse_path(path)?;

        if segments.len() == 1 {
            // Single segment: operate directly on self
            return match &segments[0] {
                PathSegment::Key(key) => match self {
                    Value::Map(map) => map.remove(key).ok_or_else(|| PathError::KeyNotFound {
                        path: path.to_string(),
                        segment: key.clone(),
                    }),
                    other => Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "map",
                        actual: other.type_name(),
                    }),
                },
                PathSegment::Index(idx) => match self {
                    Value::List(list) => {
                        if *idx >= list.len() {
                            Err(PathError::IndexOutOfBounds {
                                path: path.to_string(),
                                index: *idx,
                                length: list.len(),
                            })
                        } else {
                            Ok(list.remove(*idx))
                        }
                    }
                    other => Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "list",
                        actual: other.type_name(),
                    }),
                },
            };
        }

        // Navigate to parent
        let parent_segments = &segments[..segments.len() - 1];
        let last = &segments[segments.len() - 1];
        let mut current = self;

        for seg in parent_segments {
            match seg {
                PathSegment::Key(key) => match current {
                    Value::Map(map) => {
                        current = map.get_mut(key).ok_or_else(|| PathError::KeyNotFound {
                            path: path.to_string(),
                            segment: key.clone(),
                        })?;
                    }
                    other => {
                        return Err(PathError::TypeMismatch {
                            path: path.to_string(),
                            expected: "map",
                            actual: other.type_name(),
                        });
                    }
                },
                PathSegment::Index(idx) => match current {
                    Value::List(list) => {
                        let len = list.len();
                        current =
                            list.get_mut(*idx)
                                .ok_or_else(|| PathError::IndexOutOfBounds {
                                    path: path.to_string(),
                                    index: *idx,
                                    length: len,
                                })?;
                    }
                    other => {
                        return Err(PathError::TypeMismatch {
                            path: path.to_string(),
                            expected: "list",
                            actual: other.type_name(),
                        });
                    }
                },
            }
        }

        // Remove from parent
        match last {
            PathSegment::Key(key) => match current {
                Value::Map(map) => map.remove(key).ok_or_else(|| PathError::KeyNotFound {
                    path: path.to_string(),
                    segment: key.clone(),
                }),
                other => Err(PathError::TypeMismatch {
                    path: path.to_string(),
                    expected: "map",
                    actual: other.type_name(),
                }),
            },
            PathSegment::Index(idx) => match current {
                Value::List(list) => {
                    if *idx >= list.len() {
                        Err(PathError::IndexOutOfBounds {
                            path: path.to_string(),
                            index: *idx,
                            length: list.len(),
                        })
                    } else {
                        Ok(list.remove(*idx))
                    }
                }
                other => Err(PathError::TypeMismatch {
                    path: path.to_string(),
                    expected: "list",
                    actual: other.type_name(),
                }),
            },
        }
    }

    // ── Type-safe path accessors ────────────────────────────────────

    pub fn str_at(&self, path: &str) -> std::result::Result<&str, PathError> {
        match self.get_path(path)? {
            Value::String(s) => Ok(s),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "string",
                actual: other.type_name(),
            }),
        }
    }

    pub fn f64_at(&self, path: &str) -> std::result::Result<f64, PathError> {
        match self.get_path(path)? {
            Value::Number(n) => Ok(*n),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "number",
                actual: other.type_name(),
            }),
        }
    }

    pub fn bool_at(&self, path: &str) -> std::result::Result<bool, PathError> {
        match self.get_path(path)? {
            Value::Bool(b) => Ok(*b),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "bool",
                actual: other.type_name(),
            }),
        }
    }

    pub fn list_at(&self, path: &str) -> std::result::Result<&[Value], PathError> {
        match self.get_path(path)? {
            Value::List(v) => Ok(v),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "list",
                actual: other.type_name(),
            }),
        }
    }

    pub fn map_at(&self, path: &str) -> std::result::Result<&BTreeMap<String, Value>, PathError> {
        match self.get_path(path)? {
            Value::Map(m) => Ok(m),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "map",
                actual: other.type_name(),
            }),
        }
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
pub struct ParsedZettel {
    pub meta: ZettelMeta,
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
    pub stale_after_days: Option<u32>,
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

// ── Consistency auto-fix types ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TitleSource {
    FirstH1(String),
    Filename(String),
}

#[derive(Debug, Clone)]
pub enum Fix {
    TagsDeduped { removed: Vec<String> },
    TagsSorted,
    TagsStrippedHash { tags: Vec<String> },
    DefaultSet { field: String, value: String },
    TitleDerived { source: TitleSource },
    KeyNormalized { old: String, new: String },
    TitleTrimmed,
    TitleCapitalized,
    H1Aligned { old_h1: String, new_h1: String },
    CrossZoneResolved { key: String, kept_zone: Zone },
    FieldRenamed { old: String, new: String },
    TypeNormalized { old: String, new: String },
}

impl Fix {
    pub fn severity(&self) -> Severity {
        match self {
            Fix::CrossZoneResolved { .. } => Severity::Error,
            Fix::DefaultSet { .. } | Fix::TitleDerived { .. } | Fix::FieldRenamed { .. } => {
                Severity::Warning
            }
            Fix::TagsDeduped { .. }
            | Fix::TagsSorted
            | Fix::TagsStrippedHash { .. }
            | Fix::KeyNormalized { .. }
            | Fix::TitleTrimmed
            | Fix::TitleCapitalized
            | Fix::H1Aligned { .. }
            | Fix::TypeNormalized { .. } => Severity::Info,
        }
    }
}

impl fmt::Display for Fix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fix::TagsDeduped { removed } => write!(f, "deduplicated tags: {}", removed.join(", ")),
            Fix::TagsSorted => write!(f, "sorted tags"),
            Fix::TagsStrippedHash { tags } => {
                write!(f, "stripped # from tags: {}", tags.join(", "))
            }
            Fix::DefaultSet { field, value } => write!(f, "set default {field}: {value}"),
            Fix::TitleDerived { source } => match source {
                TitleSource::FirstH1(h) => write!(f, "derived title from H1: {h}"),
                TitleSource::Filename(n) => write!(f, "derived title from filename: {n}"),
            },
            Fix::KeyNormalized { old, new } => write!(f, "normalized key {old} -> {new}"),
            Fix::TitleTrimmed => write!(f, "trimmed title"),
            Fix::TitleCapitalized => write!(f, "capitalized title"),
            Fix::H1Aligned { old_h1, new_h1 } => {
                write!(f, "aligned H1: {old_h1} -> {new_h1}")
            }
            Fix::CrossZoneResolved { key, kept_zone } => {
                write!(
                    f,
                    "resolved cross-zone duplicate: {key} (kept {kept_zone:?})"
                )
            }
            Fix::FieldRenamed { old, new } => write!(f, "renamed field {old} -> {new}"),
            Fix::TypeNormalized { old, new } => write!(f, "normalized type {old} -> {new}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ZettelFix {
    pub path: String,
    pub applied: Vec<Fix>,
}

#[derive(Debug, Clone, Default)]
pub struct FixReport {
    pub files_scanned: usize,
    pub files_fixed: usize,
    pub fixes: Vec<ZettelFix>,
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
pub struct StaleZettel {
    pub id: String,
    pub title: String,
    pub zettel_type: String,
    pub last_updated: String,
    pub date_source: DateSource,
    pub days_stale: u32,
    pub threshold_days: u32,
}

#[derive(Debug, Clone)]
pub struct OrphanZettel {
    pub id: String,
    pub title: String,
    pub zettel_type: String,
    pub outgoing_links: usize,
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

    // ── Path navigation tests ───────────────────────────────────────

    fn nested_map() -> Value {
        let mut inner = BTreeMap::new();
        inner.insert("name".to_string(), Value::String("Alice".to_string()));
        inner.insert("age".to_string(), Value::Number(30.0));

        let mut deep = BTreeMap::new();
        deep.insert("city".to_string(), Value::String("NYC".to_string()));
        inner.insert("address".to_string(), Value::Map(deep));

        let mut root = BTreeMap::new();
        root.insert("author".to_string(), Value::Map(inner));
        root.insert(
            "tags".to_string(),
            Value::List(vec![
                Value::String("rust".to_string()),
                Value::String("zettel".to_string()),
            ]),
        );
        Value::Map(root)
    }

    #[test]
    fn path_parse_simple() {
        let segs = parse_path("a.b").unwrap();
        assert_eq!(
            segs,
            vec![PathSegment::Key("a".into()), PathSegment::Key("b".into())]
        );
    }

    #[test]
    fn path_parse_index() {
        let segs = parse_path("a[0]").unwrap();
        assert_eq!(
            segs,
            vec![PathSegment::Key("a".into()), PathSegment::Index(0)]
        );
    }

    #[test]
    fn path_parse_complex() {
        let segs = parse_path("a[0].b.c[2]").unwrap();
        assert_eq!(
            segs,
            vec![
                PathSegment::Key("a".into()),
                PathSegment::Index(0),
                PathSegment::Key("b".into()),
                PathSegment::Key("c".into()),
                PathSegment::Index(2),
            ]
        );
    }

    #[test]
    fn path_parse_escaped_dot() {
        let segs = parse_path(r"a\.b").unwrap();
        assert_eq!(segs, vec![PathSegment::Key("a.b".into())]);
    }

    #[test]
    fn path_parse_empty_rejected() {
        assert!(parse_path("").is_err());
        assert!(parse_path("a..b").is_err());
    }

    #[test]
    fn path_parse_trailing_dot_rejected() {
        assert!(parse_path("a.").is_err());
        assert!(parse_path("a.b.").is_err());
    }

    #[test]
    fn get_path_nested_map() {
        let v = nested_map();
        assert_eq!(
            v.get_path("author.name").unwrap(),
            &Value::String("Alice".into())
        );
        assert_eq!(
            v.get_path("author.address.city").unwrap(),
            &Value::String("NYC".into())
        );
    }

    #[test]
    fn get_path_list_index() {
        let v = nested_map();
        assert_eq!(
            v.get_path("tags[0]").unwrap(),
            &Value::String("rust".into())
        );
        assert_eq!(
            v.get_path("tags[1]").unwrap(),
            &Value::String("zettel".into())
        );
    }

    #[test]
    fn get_path_missing_key() {
        let v = nested_map();
        let err = v.get_path("author.email").unwrap_err();
        match err {
            PathError::KeyNotFound { segment, .. } => assert_eq!(segment, "email"),
            other => panic!("expected KeyNotFound, got {other}"),
        }
    }

    #[test]
    fn get_path_out_of_bounds() {
        let v = nested_map();
        let err = v.get_path("tags[5]").unwrap_err();
        match err {
            PathError::IndexOutOfBounds { index, length, .. } => {
                assert_eq!(index, 5);
                assert_eq!(length, 2);
            }
            other => panic!("expected IndexOutOfBounds, got {other}"),
        }
    }

    #[test]
    fn get_path_type_mismatch() {
        let v = nested_map();
        let err = v.get_path("author.name.foo").unwrap_err();
        match err {
            PathError::TypeMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, "map");
                assert_eq!(actual, "string");
            }
            other => panic!("expected TypeMismatch, got {other}"),
        }
    }

    #[test]
    fn set_path_creates_intermediates() {
        let mut v = Value::Map(BTreeMap::new());
        v.set_path("a.b.c", Value::Number(42.0)).unwrap();
        assert_eq!(v.get_path("a.b.c").unwrap(), &Value::Number(42.0));
    }

    #[test]
    fn set_path_replaces_existing() {
        let mut v = nested_map();
        v.set_path("author.name", Value::String("Bob".into()))
            .unwrap();
        assert_eq!(
            v.get_path("author.name").unwrap(),
            &Value::String("Bob".into())
        );
    }

    #[test]
    fn remove_path_returns_value() {
        let mut v = nested_map();
        let removed = v.remove_path("author.age").unwrap();
        assert_eq!(removed, Value::Number(30.0));
        assert!(v.get_path("author.age").is_err());
    }

    #[test]
    fn convenience_str_at() {
        let v = nested_map();
        assert_eq!(v.str_at("author.name").unwrap(), "Alice");
        let err = v.str_at("author.age").unwrap_err();
        match err {
            PathError::TypeMismatch { expected, .. } => assert_eq!(expected, "string"),
            other => panic!("expected TypeMismatch, got {other}"),
        }
    }

    #[test]
    fn convenience_f64_at() {
        let v = nested_map();
        assert_eq!(v.f64_at("author.age").unwrap(), 30.0);
        assert!(matches!(
            v.f64_at("author.name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "number",
                ..
            }
        ));
    }

    #[test]
    fn convenience_bool_at() {
        let mut v = Value::Map(BTreeMap::new());
        v.set_path("flag", Value::Bool(true)).unwrap();
        assert!(v.bool_at("flag").unwrap());
        v.set_path("name", Value::String("x".into())).unwrap();
        assert!(matches!(
            v.bool_at("name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "bool",
                ..
            }
        ));
    }

    #[test]
    fn convenience_list_at() {
        let v = nested_map();
        let tags = v.list_at("tags").unwrap();
        assert_eq!(tags.len(), 2);
        assert!(matches!(
            v.list_at("author.name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "list",
                ..
            }
        ));
    }

    #[test]
    fn convenience_map_at() {
        let v = nested_map();
        let author = v.map_at("author").unwrap();
        assert!(author.contains_key("name"));
        assert!(matches!(
            v.map_at("author.name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "map",
                ..
            }
        ));
    }

    #[test]
    fn round_trip() {
        let mut v = Value::Map(BTreeMap::new());
        let val = Value::List(vec![Value::Number(1.0), Value::Number(2.0)]);
        v.set_path("data.items", val.clone()).unwrap();
        assert_eq!(v.get_path("data.items").unwrap(), &val);
        assert_eq!(v.f64_at("data.items[0]").unwrap(), 1.0);
    }
}
