# ZettelDB Code Walkthrough

*2026-03-03T12:56:13Z by Showboat 0.6.1*
<!-- showboat-id: 6a329743-a932-4391-99be-fc73b1b13c90 -->

ZettelDB is a hybrid Git-CRDT decentralized Zettelkasten database written in Rust. Git stores the source of truth — plain Markdown files — while a derived SQLite index provides fast full-text search and SQL queries. When two devices edit the same zettel offline, Automerge CRDTs resolve conflicts automatically at the zone level (frontmatter, body, references).

The codebase is a Cargo workspace with four product crates plus one external e2e test package:

```bash
cat Cargo.toml
```

```toml
[workspace]
members = ["zdb-core", "zdb-cli", "zdb-server", "zdb-uniffi-bindgen", "tests"]
default-members = ["zdb-core", "zdb-cli", "zdb-server"]
resolver = "2"
```

```toml
[alias]
"test-ci" = ["test", "--workspace", "--exclude", "zdb-e2e", "--exclude", "zdb-uniffi-bindgen", "--lib", "--bins"]
"test-full" = ["test", "--workspace"]
```

- **zdb-core** — the library crate with all domain logic (parser, git, CRDT, index, SQL, sync, compaction)
- **zdb-cli** — the `zdb` binary (clap CLI)
- **zdb-server** — async GraphQL + PostgreSQL wire-protocol server (axum, async-graphql, pgwire)
- **zdb-uniffi-bindgen** — isolated UniFFI binding generator (keeps `uniffi_bindgen` out of normal builds)
- **tests** — e2e test harness (assert_cmd + reqwest)

All public API surfaces live in zdb-core. The other crates are thin consumers.

`default-members` keeps the everyday `cargo build` and `cargo test` loop focused on the library, CLI, and server crates. The external e2e harness (`zdb-e2e`) and the UniFFI bindgen helper are still available through `cargo test-full` / `cargo test --workspace`, but they no longer slow every inner-loop run. The `cargo test-ci` alias goes one step further for the cross-platform matrix: it runs only workspace library and binary targets, leaving the heavier core integration tests to a single Linux job.

## 1. Library Root — `zdb-core/src/lib.rs`

The library root declares all modules and sets up UniFFI scaffolding for generating Swift/Kotlin bindings:

```bash
cat zdb-core/src/lib.rs
```

```rust
//! # zdb-core
//!
//! Core library for Doogat ZettelDB — a hybrid Git-CRDT decentralized Zettelkasten database.
//!
//! ## Modules
//!
//! - [`parser`] — Parse and serialize three-zone Markdown zettels
//! - [`git_ops`] — Git repository operations (CRUD, merge, remote sync)
//! - [`crdt_resolver`] — Automerge CRDT conflict resolution
//! - [`indexer`] — SQLite FTS5 search index, type inference, materialization
//! - [`sql_engine`] — SQL DDL/DML translation (tables as zettel types)
//! - [`bundled_types`] — Built-in type definition templates (project, contact)
//! - [`sync_manager`] — Multi-device sync orchestration
//! - [`compaction`] — CRDT temp cleanup and git gc
//! - [`types`] — Shared data structures
//! - [`error`] — Error types and Result alias

uniffi::setup_scaffolding!();

pub mod bundle;
pub mod bundled_types;
pub mod compaction;
pub mod crdt_resolver;
pub mod error;
pub mod ffi;
pub mod git_ops;
pub mod hlc;
pub mod indexer;
pub mod parser;
pub mod sql_engine;
pub mod sync_manager;
pub mod traits;
pub mod types;

#[cfg(feature = "nosql")]
pub mod nosql;
```

`uniffi::setup_scaffolding!()` generates the FFI glue needed for the Swift/Kotlin bindings. Everything else is module declarations — no logic lives here.

## 2. Error Handling — `zdb-core/src/error.rs`

A single `ZettelError` enum covers every failure mode across all modules. The `thiserror` crate derives `Display` and `Error`. A type alias `Result<T>` is used everywhere so modules never write `std::result::Result<T, ZettelError>` longhand:

```bash
cat zdb-core/src/error.rs
```

```rust
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ZettelError>;

#[derive(Debug, Error)]
pub enum ZettelError {
    #[error("git: {0}")]
    Git(String),

    #[error("yaml: {0}")]
    Yaml(String),

    #[error("sql: {0}")]
    Sql(String),

    #[error("automerge: {0}")]
    Automerge(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml: {0}")]
    Toml(String),

    #[error("parse: {0}")]
    Parse(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation: {0}")]
    Validation(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("sql engine: {0}")]
    SqlEngine(String),

    #[error("version mismatch: repo format v{repo}, driver supports up to v{driver}")]
    VersionMismatch { repo: u32, driver: u32 },

    #[cfg(feature = "nosql")]
    #[error("redb: {0}")]
    Redb(String),
}
```

Each upstream library gets its own `From` impl in the module that uses it (e.g. `From<git2::Error>` in `git_ops.rs`, `From<rusqlite::Error>` in `indexer.rs`). The `VersionMismatch` variant handles the forward-compatibility check when opening repos created by newer ZettelDB versions. The `InvalidPath` variant is raised by `validate_path()` in `git_ops.rs` when a file path contains `..` traversal components or is a symlink.

## 3. Domain Types — `zdb-core/src/types.rs`

This file defines every data structure shared across modules. The key types form a clear hierarchy:

```bash
sed -n '108,194p' zdb-core/src/types.rs
```

```rust
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParsedZettel {
    pub meta: ZettelMeta,
    pub body: String,
    pub reference_section: String,
    pub inline_fields: Vec<InlineField>,
    pub links: Vec<Link>,
    pub body_tags: Vec<String>,
    pub checkboxes: Vec<CheckboxItem>,
    pub path: String,
}
```

**ZettelId** is a 14-digit timestamp string (YYYYMMDDHHmmss). The custom `Deserialize` impl handles YAML quirks where `20260226120000` might parse as an integer rather than a string.

**Zone** enum tags which section of the markdown file something belongs to — critical for the per-zone CRDT merge strategy.

**ZettelMeta** holds the structured frontmatter. Known fields (`id`, `title`, `date`, `type`, `tags`) are first-class; everything else lands in `extra` as a generic `Value` enum. This keeps the system extensible — users can add arbitrary YAML fields.

**ParsedZettel** is the fully-parsed representation of a zettel file. It carries metadata, body text, reference section, extracted inline fields (Dataview-style `key:: value`), links of all kinds (wikilinks, embeds, markdown links, bare URLs), body hashtags, and checkboxes. This struct flows through the entire system — created by the parser, persisted via git, indexed in SQLite, served over GraphQL.

The file also defines types for the sync and merge subsystems:

```bash
sed -n '196,290p' zdb-core/src/types.rs
```

```rust
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
}

#[derive(Debug, Clone, Default)]
pub struct CompactOptions {
    pub force: bool,
    pub skip_backup: bool,
    pub backup_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct CompactionReport {
    pub files_removed: usize,
    pub crdt_docs_compacted: usize,
    pub gc_success: bool,
    // ... byte/file stats ...
    pub backup_path: Option<PathBuf>,
}
```

**Zettel** is the raw three-zone split (before YAML parsing). **ConflictFile** carries both sides of a git merge conflict plus optional HLC timestamps for each side. **MergeResult** is the outcome of `git merge` — the `Conflicts` variant includes the list of conflicting files and the commit hash of "theirs".

**NodeConfig** tracks a sync node (device). Each device gets a UUID, a human name, and lifecycle status (Active → Stale → Retired). The `known_heads` field records the last git commits this node has seen, enabling incremental sync.

## 4. Trait Abstractions — `zdb-core/src/traits.rs`

Four traits define the system's boundaries, enabling dependency inversion and testability:

```bash
sed -n '1,38p' zdb-core/src/traits.rs
```

```rust
use crate::error::Result;
use crate::types::{
    CommitHash, ConflictFile, DiffKind, PaginatedSearchResult, ParsedZettel, ResolvedFile,
    SearchResult,
};

/// Read-only access to zettel storage.
pub trait ZettelSource {
    fn list_zettels(&self) -> Result<Vec<String>>;
    fn read_file(&self, path: &str) -> Result<String>;
    fn head_oid(&self) -> Result<CommitHash>;
    fn diff_paths(&self, old_oid: &str, new_oid: &str) -> Result<Vec<(DiffKind, String)>>;
}

/// Read-write access to zettel storage.
pub trait ZettelStore: ZettelSource {
    fn commit_file(&self, path: &str, content: &str, msg: &str) -> Result<CommitHash>;
    fn commit_files(&self, files: &[(&str, &str)], msg: &str) -> Result<CommitHash>;
    fn delete_file(&self, path: &str, msg: &str) -> Result<CommitHash>;
}

/// Query and mutation operations on the zettel index.
pub trait ZettelIndex {
    fn index_zettel(&self, zettel: &ParsedZettel) -> Result<()>;
    fn remove_zettel(&self, id: &str) -> Result<()>;
    fn search(&self, query: &str) -> Result<Vec<SearchResult>>;
    fn search_paginated(&self, query: &str, limit: usize, offset: usize) -> Result<PaginatedSearchResult>;
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
```

**ZettelSource** provides read-only git access (list files, read content, get HEAD, diff between commits). `diff_paths` uses `git2::diff_tree_to_tree` to compute changed paths between two commits — this powers incremental reindex. **ZettelStore** extends it with write operations (commit, delete). This split lets the indexer rebuild from a read-only source while the SQL engine needs writes for INSERT/UPDATE/DELETE.

**ZettelIndex** abstracts the SQLite layer — both the search index and the `_zdb_*` metadata tables.

**ConflictResolver** is the strategy interface for CRDT merge. `GitRepo` implements `ZettelSource + ZettelStore`. `Index` implements `ZettelIndex`. `DefaultResolver` implements `ConflictResolver`. Unit tests use `MockSource` (also in `traits.rs`) for isolated testing without git.

## 5. Three-Zone Markdown Parser — `zdb-core/src/parser.rs`

Every zettel is a markdown file split into three zones separated by `---`:

```
---
id: 20260226120000
title: My Note
tags:
  - rust
  - crdt
---
Body content with [[wikilinks]] and key:: inline fields
---
- source:: Wikipedia
- related:: [[20260101000000]]
```

The parser's main entry point is `parse()`, which orchestrates the full pipeline:

```bash
sed -n '407,423p' zdb-core/src/parser.rs
```

```rust
/// Parse a zettel Markdown file into a fully structured ParsedZettel.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn parse(content: &str, path: &str) -> Result<crate::types::ParsedZettel> {
    let zettel = split_zones(content)?;
    let meta = parse_frontmatter(&zettel.raw_frontmatter, path)?;
    let inline_fields = extract_inline_fields(&zettel.body, &zettel.reference_section)?;
    let wikilinks = extract_links(
        &zettel.raw_frontmatter,
        &zettel.body,
        &zettel.reference_section,
    );
    let body_tags = extract_hashtags(&zettel.body);
    let checkboxes = extract_checkboxes(&zettel.body);

    Ok(crate::types::ParsedZettel {
        meta,
        body: zettel.body,
        reference_section: zettel.reference_section,
        inline_fields,
        links: wikilinks,
        body_tags,
        checkboxes,
        path: path.to_string(),
    })
}
```

Six steps: `split_zones` → `parse_frontmatter` → `extract_inline_fields` → `extract_links` → `extract_hashtags` → `extract_checkboxes`.

`extract_links()` is the unified link extraction pipeline. For each zone (frontmatter, body, reference), it runs four extractors in order: embeds → wikilinks → markdown links → bare URLs. Embeds are extracted first so the wikilink pass can filter out overlapping `[[...]]` matches. Markdown link targets are collected so the bare URL pass can skip URLs that already appear as markdown link targets. Each link carries a `LinkKind` discriminant (`WikiLink`, `Embed`, `MarkdownLink`, `BareUrl`).

### Zone Splitting

`split_zones()` is the trickiest part. It finds the frontmatter `---` pair, then scans backward from the end looking for a valid reference boundary — a `---` where ALL non-empty lines below it match `- key:: value`:

```bash
sed -n '91,148p' zdb-core/src/parser.rs
```

```rust
/// Split markdown content into three zones: frontmatter, body, reference section.
///
/// Heuristic for reference section: find last `---` on its own line (after frontmatter);
/// if ALL non-empty lines after it match `- key:: value` pattern, that's the boundary.
/// Backtracks if content after last `---` is empty/whitespace.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn split_zones(content: &str) -> Result<Zettel> {
    let lines: Vec<&str> = content.lines().collect();

    // Find frontmatter boundaries (first `---` pair), tracking fenced code blocks
    let (fm_start, fm_end) = find_frontmatter(&lines)?;

    let frontmatter = lines[fm_start + 1..fm_end].join("\n");

    // Collect all `---` positions after frontmatter, skipping those inside fenced code blocks
    let separator_positions = find_separators_after(&lines, fm_end);

    // Try separators from last to first, looking for valid reference boundary.
    // When backtracking, check content between this separator and the next one (or EOF).
    let mut ref_boundary = None;
    let mut end_boundary = lines.len(); // exclusive upper bound for reference content
    for &pos in separator_positions.iter().rev() {
        let after = &lines[pos + 1..end_boundary];
        if after.iter().all(|l| l.trim().is_empty()) {
            // Empty/whitespace only → skip this separator and narrow the window
            end_boundary = pos;
            continue;
        }
        if after
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| is_reference_line(l))
        {
            ref_boundary = Some(pos);
            break;
        }
        // Content doesn't match reference pattern → stop searching
        break;
    }

    let (body, reference_section) = match ref_boundary {
        Some(pos) => {
            let body = lines[fm_end + 1..pos].join("\n");
            let reference = lines[pos + 1..end_boundary].join("\n");
            (body, reference)
        }
        None => {
            let body = lines[fm_end + 1..].join("\n");
            (body, String::new())
        }
    };

    Ok(Zettel {
        raw_frontmatter: frontmatter,
        body,
        reference_section,
    })
}
```

The backward scan is key. A `---` in the body might be a thematic break (markdown horizontal rule), not a reference boundary. The heuristic: scan `---` markers from last to first. If the content below a marker is all `- key:: value` lines, that's the reference section. If it's empty, backtrack to the previous marker. If it's normal prose, stop — there's no reference section.

Fenced code blocks (``` or ~~~) are tracked to avoid false positives on `---` inside code.

### Inline Fields and Wikilinks

After zone splitting, the parser extracts two kinds of structured content:

```bash
sed -n '210,257p' zdb-core/src/parser.rs
```

```rust
/// Extract Dataview-style inline fields from body and reference zones.
/// Body fields: `key:: value` on a line. Reference fields: `- key:: value` (list-item).
/// Cross-zone duplicate keys → validation error. Same-zone duplicates: first wins silently.
pub fn extract_inline_fields(body: &str, reference: &str) -> crate::error::Result<Vec<InlineField>> {
    use std::sync::OnceLock;
    static BODY_RE: OnceLock<Regex> = OnceLock::new();
    static REF_RE: OnceLock<Regex> = OnceLock::new();

    static INLINE_CODE_RE: OnceLock<Regex> = OnceLock::new();
    static FENCE_RE: OnceLock<Regex> = OnceLock::new();

    let body_re = BODY_RE.get_or_init(|| Regex::new(r"^([\w][\w\s-]*):: (.+)$").expect("valid regex: body inline field"));
    let ref_re = REF_RE.get_or_init(|| Regex::new(r"^- ([\w][\w\s-]*):: ?(.*)$").expect("valid regex: ref inline field"));
    let inline_code_re = INLINE_CODE_RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid regex: inline code"));
    let fence_re = FENCE_RE.get_or_init(|| Regex::new(r"^(?:`{3,}|~{3,})").expect("valid regex: fence marker"));

    let mut fields = Vec::new();
    let mut seen: std::collections::HashMap<String, Zone> = std::collections::HashMap::new();
    let mut in_fence = false;

    for line in body.lines() {
        if fence_re.is_match(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let stripped = inline_code_re.replace_all(line, "");
        if let Some(caps) = body_re.captures(&stripped) {
            let key = caps[1].trim().to_string();
            match seen.get(&key) {
                Some(Zone::Body) => {} // same-zone dup, first wins
                Some(_) => {
                    return Err(crate::error::ZettelError::Validation(format!(
                        "duplicate inline field '{key}' across body and reference zones"
                    )));
                }
                None => {
                    seen.insert(key.clone(), Zone::Body);
                    fields.push(InlineField {
                        key,
                        value: caps[2].to_string(),
                        zone: Zone::Body,
                    });
                }
            }
        }
```

**Inline fields** use Dataview syntax (`key:: value` in body, `- key:: value` in references). The parser strips inline code before matching to avoid false positives on `key:: value` inside backticks. Cross-zone duplicate keys are an error; same-zone duplicates silently keep the first occurrence.

**Wikilinks** (`[[target|display]]`) are extracted from all three zones with a single regex.

**Hashtags** (`#tag/subtag`) are extracted from body text via `extract_hashtags()`. The function reuses the same fence-tracking and inline-code-stripping logic as inline fields, and additionally strips wikilinks (`[[...]]`) before matching. URL fragments (`https://example.com#section`) are excluded by checking for `://` before the `#`. Tags are deduplicated and stored in `ParsedZettel.body_tags`. The indexer inserts them into `_zdb_tags` with `source = 'body'` alongside frontmatter tags (`source = 'frontmatter'`).

**Checkboxes** (`- [ ]`, `- [x]`, `- [i]`) are extracted from body text via `extract_checkboxes()`. Each item captures state (open/done/info), content text, optional date prefix (`YYYY-MM-DD HH:MM`), optional due date (`⏳ YYYY-MM-DD`), line number, and indentation level (number of leading spaces). Fenced code blocks are excluded. Indentation tracks sub-item nesting depth. Results stored in `ParsedZettel.checkboxes` and indexed in `_zdb_checkboxes` table, enabling GTD queries like `SELECT * FROM _zdb_checkboxes WHERE state = 'open'`.

### ID Generation

ZettelDB uses 14-digit timestamp IDs with collision prevention:

```bash
sed -n '425,450p' zdb-core/src/parser.rs
```

```rust
/// Generate a zettel ID from the current local timestamp (YYYYMMDDHHmmss).
/// Generate a 14-digit timestamp ID (YYYYMMDDHHmmss).
///
/// Within a single process, consecutive calls in the same second will
/// spin-wait until the clock advances, preventing collisions.
pub fn generate_id() -> ZettelId {
    generate_unique_id(|_| false)
}

/// Generate a unique 14-digit timestamp ID, spin-waiting if `exists`
/// returns true for the candidate. Also deduplicates within-process.
pub fn generate_unique_id(exists: impl Fn(&str) -> bool) -> ZettelId {
    use std::sync::Mutex;
    static LAST: Mutex<String> = Mutex::new(String::new());

    let mut last = LAST.lock().unwrap();
    loop {
        let now = chrono::Local::now();
        let candidate = now.format("%Y%m%d%H%M%S").to_string();
        if candidate != *last && !exists(&candidate) {
            *last = candidate.clone();
            return ZettelId(candidate);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
```

A `static Mutex<String>` tracks the last generated ID within the process. If the clock hasn't advanced (same second), it spin-waits 100ms. The `exists` callback lets callers check the filesystem for cross-process collisions. The CLI passes a closure that scans `zettelkasten/` and its subdirectories.

## 6. Git Storage — `zdb-core/src/git_ops.rs`

`GitRepo` wraps `git2::Repository` and provides the storage layer. Every mutation is a git commit:

```bash
sed -n '20,69p' zdb-core/src/git_ops.rs
```

```rust
pub struct GitRepo {
    pub repo: Repository,
    pub path: PathBuf,
}

impl GitRepo {
    /// Initialize a new zettelkasten Git repository.
    pub fn init(path: &Path) -> Result<Self> {
        let repo = Repository::init(path)?;
        let git_repo = Self {
            repo,
            path: path.to_path_buf(),
        };

        // Create standard directories with .gitkeep
        for dir in &["zettelkasten", "reference", ".nodes", ".crdt/temp"] {
            let dir_path = path.join(dir);
            std::fs::create_dir_all(&dir_path)?;
            std::fs::write(dir_path.join(".gitkeep"), "")?;
        }

        // Add .zdb/ to .gitignore
        let gitignore_path = path.join(".gitignore");
        let existing = if gitignore_path.exists() {
            std::fs::read_to_string(&gitignore_path)?
        } else {
            String::new()
        };
        if !existing.contains(".zdb/") {
            let content = if existing.is_empty() {
                ".zdb/\n".to_string()
            } else {
                format!("{existing}\n.zdb/\n")
            };
            std::fs::write(&gitignore_path, content)?;
        }

        // Write format version file
        std::fs::write(path.join(VERSION_FILE), CURRENT_FORMAT_VERSION.to_string())?;

        // Write default repo config
        let default_config = RepoConfig::default();
        let config_toml = toml::to_string_pretty(&default_config)
            .map_err(|e| ZettelError::Toml(e.to_string()))?;
        std::fs::write(path.join(CONFIG_FILE), &config_toml)?;

        // Stage everything and create initial commit
        git_repo.commit_all("init: zettelkasten repository")?;

        Ok(git_repo)
```

`init()` creates the directory layout: `zettelkasten/` for zettels, `reference/` for attachments, `.nodes/` for sync node configs, `.crdt/temp/` for temporary Automerge documents. The `.zdb/` directory (SQLite index) is gitignored — it's a derived cache.

A `.zetteldb-version` file tracks the on-disk format version. On `open()`, the driver checks this version: if the repo is newer, it rejects it; if older, it auto-migrates.

### Commits and Reads

Every mutation goes through `commit_files()` — write to disk, stage, commit:

```bash
sed -n '174,204p' zdb-core/src/git_ops.rs
```

```rust
    /// Write a file, stage it, and commit.
    pub fn commit_file(&self, rel_path: &str, content: &str, message: &str) -> Result<CommitHash> {
        self.commit_files(&[(rel_path, content)], message)
    }

    /// Write multiple files, stage them, and commit.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn commit_files(&self, files: &[(&str, &str)], message: &str) -> Result<CommitHash> {
        for (rel_path, content) in files {
            let full_path = self.path.join(rel_path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, content)?;
        }

        let mut index = self.repo.index()?;
        for (rel_path, _) in files {
            index.add_path(Path::new(rel_path))?;
        }
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;

        let parent = self.head_commit()
            .ok_or_else(|| ZettelError::Git("repo has no initial commit".into()))?;
        let oid = self.repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        self.write_commit_graph();
        Ok(CommitHash(oid.to_string()))
    }
```

After each commit, `write_commit_graph()` calls `git commit-graph write --reachable` to speed up future merge-base calculations. Reads always go through the HEAD tree (not the working directory), keeping the git object database as the single source of truth:

```bash
sed -n '445,457p' zdb-core/src/git_ops.rs
```

```rust
    /// Read file content from HEAD tree.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn read_file(&self, rel_path: &str) -> Result<String> {
        let head = self.repo.head()?.peel_to_commit()?;
        let tree = head.tree()?;
        let entry = tree.get_path(Path::new(rel_path))
            .map_err(|_| ZettelError::NotFound(rel_path.to_string()))?;
        let blob = self.repo.find_blob(entry.id())
            .map_err(|_| ZettelError::NotFound(rel_path.to_string()))?;
        let content = std::str::from_utf8(blob.content())
            .map_err(|e| ZettelError::Parse(e.to_string()))?;
        Ok(content.to_string())
    }
```

### Path Validation

All file I/O in `git_ops.rs` passes through `validate_path()` before touching the filesystem. This guards against three attack vectors: absolute paths (which would replace the base in `Path::join`), symlink dereferencing (a symlink under `zettelkasten/` pointing outside the repo), and path traversal (`../` components escaping the repo root).

```bash
sed -n '20,50p' zdb-core/src/git_ops.rs
```

```rust
/// Reject symlinks, absolute paths, and paths that escape the repository root.
///
/// Works for both existing and not-yet-created paths:
/// 1. Rejects absolute paths (which would replace the base in `Path::join`).
/// 2. Component check catches `..` traversal regardless of file existence.
/// 3. For paths that exist on disk, also rejects symlinks and verifies
///    the canonical path stays within the repo root.
fn validate_path(repo_root: &Path, relative: &str) -> Result<()> {
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err(ZettelError::InvalidPath(format!(
            "absolute paths not allowed: {relative}"
        )));
    }
    for component in rel.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(ZettelError::InvalidPath(format!(
                "path escapes repository root: {relative}"
            )));
        }
    }

    let full = repo_root.join(relative);
    if let Ok(meta) = full.symlink_metadata() {
        if meta.file_type().is_symlink() {
            return Err(ZettelError::InvalidPath(format!(
                "symlinks not allowed: {relative}"
            )));
        }
        let canonical = full.canonicalize()?;
        let root_canonical = repo_root.canonicalize()?;
        if !canonical.starts_with(&root_canonical) {
            return Err(ZettelError::InvalidPath(format!(
                "path escapes repository root: {relative}"
            )));
        }
    }

    Ok(())
}
```

The function has three layers of defense. First, it rejects absolute paths outright — in Rust, `Path::join` with an absolute path replaces the base entirely, which would bypass all subsequent checks. Second, a path-component scan rejects any `..` segments — this works even for paths that don't exist on disk yet (new file writes, git tree reads). Third, for paths that already exist on the filesystem, it checks `symlink_metadata()` to reject symlinks and `canonicalize()` to verify the resolved path stays under the repo root.

Every public method that performs file I/O calls `validate_path` before proceeding: `read_file`, `commit_file`, `commit_files`, `commit_binary_file`, `commit_binary_and_text`, `commit_merge`, `commit_batch`, `delete_file`, and `delete_files`. The `InvalidPath` error variant surfaces through the FFI layer as a `Validation` error and through the REST/GraphQL layer as a `400 Bad Request`.

### Merging

The `merge_remote()` method handles three-way merges when syncing:

```bash
sed -n '285,343p' zdb-core/src/git_ops.rs
```

```rust
    /// Merge a fetched remote branch, returning the merge result.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn merge_remote(&self, remote: &str, branch: &str) -> Result<MergeResult> {
        let fetch_head_ref = format!("refs/remotes/{remote}/{branch}");
        let reference = self.repo.find_reference(&fetch_head_ref)
            .map_err(|_| ZettelError::NotFound(fetch_head_ref.clone()))?;
        let annotated = self.repo.reference_to_annotated_commit(&reference)?;

        let (analysis, _pref) = self.repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(MergeResult::AlreadyUpToDate);
        }

        if analysis.is_fast_forward() {
            let target_oid = annotated.id();
            let mut reference = self.repo.find_reference("refs/heads/master")
                .or_else(|_| self.repo.find_reference("HEAD"))?;
            reference.set_target(target_oid, "fast-forward")?;
            self.repo.set_head("refs/heads/master")?;
            self.repo.checkout_head(Some(
                git2::build::CheckoutBuilder::new().force(),
            ))?;
            self.write_commit_graph();
            return Ok(MergeResult::FastForward(CommitHash(target_oid.to_string())));
        }

        // Normal merge
        let their_commit = self.repo.find_commit(annotated.id())?;
        let our_commit = self.head_commit().ok_or_else(|| ZettelError::Parse("no HEAD".into()))?;
        let _ancestor = self.repo.merge_base(our_commit.id(), their_commit.id())?;

        let mut merge_index = self.repo.merge_commits(&our_commit, &their_commit, None)?;

        if merge_index.has_conflicts() {
            let conflicts = self.extract_conflicts(&merge_index, &our_commit, &their_commit)?;
            // Clean up merge state
            self.repo.cleanup_state()?;
            return Ok(MergeResult::Conflicts(conflicts, CommitHash(their_commit.id().to_string())));
        }

        // Clean merge — write tree and commit
        let tree_oid = merge_index.write_tree_to(&self.repo)?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = self.signature()?;
        let oid = self.repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("merge {remote}/{branch}"),
            &tree,
            &[&our_commit, &their_commit],
        )?;
        self.repo.checkout_head(Some(
            git2::build::CheckoutBuilder::new().force(),
        ))?;
        self.write_commit_graph();
        Ok(MergeResult::Clean(CommitHash(oid.to_string())))
    }
```

The merge follows standard git semantics: up-to-date → fast-forward → 3-way merge. Conflicts are extracted with full content for all three sides (ancestor, ours, theirs) and passed upstream to the CRDT resolver. Git merge state is cleaned up immediately after extracting conflicts — the CRDT layer handles resolution, not git.

`extract_conflicts()` also populates each `ConflictFile`'s `ours_hlc` and `theirs_hlc` by calling `find_hlc_for_path()`, which walks commit ancestry to find the most recent commit touching each path and extracts the HLC trailer. This enables accurate LWW fallback when CRDT resolution fails.

`find_hlc_for_path()` is hardened against bad commits: if `find_commit()`, `tree()`, or `diff_tree_to_tree()` fails mid-walk, the error is logged and the commit is skipped rather than aborting the search. A depth limit of 100 commits bounds the walk to prevent pathological slowdowns on large histories; when exceeded, `None` is returned with a warning.

`GitRepo` implements both `ZettelSource` and `ZettelStore` traits, delegating directly to the inherent methods.

## 7. Hybrid Logical Clock — `zdb-core/src/hlc.rs`

The HLC provides causally-ordered timestamps across distributed nodes. It combines a wall clock, a logical counter, and a node ID:

```bash
sed -n '1,72p' zdb-core/src/hlc.rs
```

```rust
use std::cmp::Ordering;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Result, ZettelError};

/// Hybrid Logical Clock — combines wall clock, logical counter, and node ID
/// for causally-ordered, conflict-free timestamps across distributed nodes.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
    pub node: String, // first 8 chars of node UUID
}

impl Hlc {
    /// Tick the clock for a local event.
    pub fn now(node_id: &str, last: &Option<Hlc>) -> Hlc {
        let wall = wall_clock_ms();
        let node = truncate_node(node_id);

        match last {
            Some(prev) => {
                if wall > prev.wall_ms {
                    Hlc { wall_ms: wall, counter: 0, node }
                } else {
                    Hlc { wall_ms: prev.wall_ms, counter: prev.counter + 1, node }
                }
            }
            None => Hlc { wall_ms: wall, counter: 0, node },
        }
    }

    /// Merge on receive: take max(local, remote, wall) and bump counter if tied.
    pub fn recv(node_id: &str, local_last: &Option<Hlc>, remote: &Hlc) -> Hlc {
        let wall = wall_clock_ms();
        let node = truncate_node(node_id);

        let local_wall = local_last.as_ref().map(|h| h.wall_ms).unwrap_or(0);
        let local_counter = local_last.as_ref().map(|h| h.counter).unwrap_or(0);

        let max_wall = wall.max(local_wall).max(remote.wall_ms);

        let counter = if max_wall == local_wall && max_wall == remote.wall_ms {
            local_counter.max(remote.counter) + 1
        } else if max_wall == local_wall {
            local_counter + 1
        } else if max_wall == remote.wall_ms {
            remote.counter + 1
        } else {
            // wall clock is strictly ahead
            0
        };

        Hlc { wall_ms: max_wall, counter, node }
    }

    /// Parse from sortable string format: `{wall_ms}-{counter:04}-{node}`.
    pub fn parse(s: &str) -> Result<Hlc> {
        let parts: Vec<&str> = s.splitn(3, '-').collect();
        if parts.len() != 3 {
            return Err(ZettelError::Parse(format!("invalid HLC: {s}")));
        }
        let wall_ms = parts[0]
            .parse::<u64>()
            .map_err(|e| ZettelError::Parse(format!("bad HLC wall_ms: {e}")))?;
        let counter = parts[1]
            .parse::<u32>()
            .map_err(|e| ZettelError::Parse(format!("bad HLC counter: {e}")))?;
        let node = parts[2].to_string();
        Ok(Hlc { wall_ms, counter, node })
    }
```

**`now()`** ticks for local events. If wall clock advanced, reset counter to 0. If not (multiple events within 1ms), increment the counter from the last HLC.

**`recv()`** merges on receiving a remote HLC. Takes `max(wall_clock, local_last, remote)` and bumps the counter when wall times tie.

**Ordering** is `wall_ms → counter → node`, giving total order. The string format (`1709000000000-0042-abcd1234`) is lexicographically sortable.

HLC timestamps are embedded in git commit messages as trailers (`HLC: 1709000000000-0001-abcd1234`) and extracted during conflict resolution to enable the Last-Writer-Wins strategy.

## 8. CRDT Conflict Resolution — `zdb-core/src/crdt_resolver.rs`

This is the heart of the decentralized merge. Three strategies are available, each suited to different zettel types.

### Strategy 1: Default (Automerge per-zone)

The default strategy resolves each zone independently:

```bash
sed -n '16,82p' zdb-core/src/crdt_resolver.rs
```

```rust
/// Resolve all conflict files using per-zone CRDT merge strategies.
/// If `crdt_strategy` is set to something other than `preset:default`, a warning is logged
/// since only the default strategy is currently implemented.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn resolve_conflicts(
    conflicts: Vec<ConflictFile>,
    crdt_strategy: Option<&str>,
) -> Result<Vec<ResolvedFile>> {
    if let Some(strategy) = crdt_strategy {
        match strategy {
            "preset:default" => {}
            "preset:last-writer-wins" => return resolve_lww(conflicts),
            "preset:append-log" => return resolve_append_log(conflicts),
            other => {
                tracing::warn!(
                    "crdt_strategy '{}' not recognized; using default",
                    other
                );
            }
        }
    }

    let mut resolved = Vec::new();

    for conflict in conflicts {
        let ancestor_content = conflict.ancestor.as_deref().unwrap_or("");

        let ancestor = parse_zones(ancestor_content)?;
        let ours = parse_zones(&conflict.ours)?;
        let theirs = parse_zones(&conflict.theirs)?;

        let (merged_fm, fm_crdt_bytes) = merge_frontmatter(
            &ancestor.raw_frontmatter,
            &ours.raw_frontmatter,
            &theirs.raw_frontmatter,
        )?;
        let merged_body = merge_body(&ancestor.body, &ours.body, &theirs.body)?;
        let merged_ref = merge_reference(
            &ancestor.reference_section,
            &ours.reference_section,
            &theirs.reference_section,
        )?;

        // Reassemble via parser
        let meta = parser::parse_frontmatter(&merged_fm, &conflict.path)?;
        let inline_fields = parser::extract_inline_fields(&merged_body, &merged_ref)?;
        let wikilinks = parser::extract_wikilinks(&merged_fm, &merged_body, &merged_ref);

        let parsed = crate::types::ParsedZettel {
            meta,
            body: merged_body,
            reference_section: merged_ref,
            inline_fields,
            links: wikilinks,
            body_tags: vec![],
            checkboxes: vec![],
            path: conflict.path.clone(),
        };

        let content = parser::serialize(&parsed);
        resolved.push(ResolvedFile {
            path: conflict.path,
            content,
            fm_crdt_bytes: Some(fm_crdt_bytes),
        });
    }

    Ok(resolved)
}
```

Each conflict file is split into three zones, then each zone is merged independently:

- **Frontmatter**: Scalar fields via Automerge Map CRDT (fork → diff → merge). List fields (like `tags`) via three-way set merge (ancestor + additions - removals).
- **Body**: Automerge Text CRDT with character-level diffs using the `similar` crate.
- **Reference**: Automerge List CRDT at line level, sorted and deduped on output.

The three merged zones are reassembled into a valid `ParsedZettel` and serialized back to markdown. The `ResolvedFile` now carries `fm_crdt_bytes: Option<Vec<u8>>` — the serialized automerge document for frontmatter state persistence.

### Frontmatter merge detail:

```bash
sed -n '101,154p' zdb-core/src/crdt_resolver.rs
```

```rust

/// Merge YAML frontmatter at field granularity.
/// Scalar fields use Automerge Map CRDT. List fields (e.g. tags) use three-way set merge.
/// Returns `(resolved_yaml, automerge_doc_bytes)` for CRDT state persistence.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn merge_frontmatter(ancestor: &str, ours: &str, theirs: &str) -> Result<(String, Vec<u8>)> {
    let ancestor_map = yaml_to_map(ancestor)?;
    let ours_map = yaml_to_map(ours)?;
    let theirs_map = yaml_to_map(theirs)?;

    // Partition into scalars and lists
    let (ancestor_scalars, ancestor_lists) = partition_fm(&ancestor_map);
    let (ours_scalars, ours_lists) = partition_fm(&ours_map);
    let (theirs_scalars, theirs_lists) = partition_fm(&theirs_map);

    // Merge scalars via Automerge Map CRDT
    let mut doc = AutoCommit::new();
    let map_id = doc.put_object(automerge::ROOT, "frontmatter", ObjType::Map)?;
    for (k, v) in &ancestor_scalars {
        doc.put(&map_id, k.as_str(), v.as_str())?;
    }

    let mut doc_ours = doc.fork();
    let ours_map_id = doc_ours.get(&automerge::ROOT, "frontmatter")?
        .map(|(_, id)| id)
        .ok_or_else(|| ZettelError::Parse("missing frontmatter map".into()))?;
    apply_scalar_diff(&mut doc_ours, &ours_map_id, &ancestor_scalars, &ours_scalars)?;

    let mut doc_theirs = doc.fork();
    let theirs_map_id = doc_theirs.get(&automerge::ROOT, "frontmatter")?
        .map(|(_, id)| id)
        .ok_or_else(|| ZettelError::Parse("missing frontmatter map".into()))?;
    apply_scalar_diff(&mut doc_theirs, &theirs_map_id, &ancestor_scalars, &theirs_scalars)?;

    doc_ours.merge(&mut doc_theirs)?;

    let merged_map_id = doc_ours.get(&automerge::ROOT, "frontmatter")?
        .map(|(_, id)| id)
        .ok_or_else(|| ZettelError::Parse("missing frontmatter map after merge".into()))?;

    let mut merged = BTreeMap::new();
    for key in doc_ours.keys(&merged_map_id) {
        if let Some((value, _)) = doc_ours.get(&merged_map_id, key.as_str())? {
            merged.insert(key, FmValue::Scalar(value.to_string()));
        }
    }

    // Merge lists via three-way set merge
    for (k, v) in merge_list_fields(&ancestor_lists, &ours_lists, &theirs_lists) {
        merged.insert(k, v);
    }

    let doc_bytes = doc_ours.save();
    Ok((map_to_yaml(&merged), doc_bytes))
```

The Automerge pattern is fork → apply diffs → merge. Starting from the ancestor state, both "ours" and "theirs" diffs are applied to separate forks, then merged. Automerge handles the convergence deterministically. After merging, `doc_ours.save()` serializes the full automerge document for persistence as `_fm.crdt`.

For list fields like `tags`, a custom three-way set merge preserves order: start with ancestor set, add items introduced by each side, remove items deleted by each side.

### Frontmatter CRDT state persistence

`merge_frontmatter()` returns `(String, Vec<u8>)` — the resolved YAML plus serialized Automerge document bytes. After conflict resolution, `sync_manager` writes the bytes to `.crdt/temp/{commit_oid}_{zettel_id}_fm.crdt`. This preserves the CRDT state separately from body CRDT files so that:

1. A node that hasn't synced past the merge commit can re-merge from the saved state
2. Compaction handles frontmatter and body CRDT files independently — `compact_crdt_docs()` groups by `(zettel_id, is_frontmatter)`, producing separate `compacted_{id}.crdt` and `compacted_{id}_fm.crdt` files
3. `cleanup_crdt_temp()` uses the same shared-head logic for both file types

The `_fm` suffix is parsed by `parse_crdt_temp_name()`, which returns `(oid, zettel_id, is_frontmatter)`.

### Strategy 2: Last-Writer-Wins (LWW)

The simplest strategy — compare HLC timestamps, pick the winner:

```bash
sed -n '484,512p' zdb-core/src/crdt_resolver.rs
```

```rust

/// Resolve conflicts using Last-Writer-Wins by HLC comparison.
/// Higher HLC wins. Tie-break: higher node string wins.
/// If no HLC available, falls back to "ours".
pub fn resolve_lww(conflicts: Vec<ConflictFile>) -> Result<Vec<ResolvedFile>> {
    let mut resolved = Vec::new();
    for conflict in conflicts {
        let content = pick_lww_winner(&conflict);
        resolved.push(ResolvedFile {
            path: conflict.path,
            content,
            fm_crdt_bytes: None,
        });
    }
    Ok(resolved)
}

/// Pick the winner based on HLC comparison.
fn pick_lww_winner(conflict: &ConflictFile) -> String {
    match (&conflict.ours_hlc, &conflict.theirs_hlc) {
        (Some(ours_hlc), Some(theirs_hlc)) => {
            if theirs_hlc > ours_hlc {
                conflict.theirs.clone()
            } else {
                conflict.ours.clone()
            }
        }
        // No HLC available — fallback to ours
        _ => conflict.ours.clone(),
```

LWW is whole-file — no per-zone merging. Higher HLC wins. Tie-break by node string (deterministic). Without HLC data, "ours" wins. This is the fallback when the default CRDT merge produces invalid output.


### Strategy 3: Append-Log

For project/journal zettels with temporal log sections (`## Log` followed by `- [x] 2026-01-01 Did thing`), entries from both sides are unioned, deduped, and sorted chronologically. Non-log body sections still use the text CRDT. Frontmatter and references use the same merge as the default strategy.

## 9. SQLite Indexer — `zdb-core/src/indexer.rs`

The index is a derived SQLite database (`~/.zdb/index.db`) providing fast queries. It's always rebuildable from git. The schema:

```bash
sed -n '17,83p' zdb-core/src/indexer.rs
```

```rust
pub struct Index {
    pub(crate) conn: Connection,
}

impl Index {
    /// Open (or create) the SQLite index database.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS zettels (
                id TEXT PRIMARY KEY,
                title TEXT,
                date TEXT,
                type TEXT,
                path TEXT UNIQUE NOT NULL,
                body TEXT,
                updated_at TEXT
            );

            CREATE TABLE IF NOT EXISTS _zdb_tags (
                zettel_id TEXT NOT NULL REFERENCES zettels(id),
                tag TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'frontmatter'
            );
            CREATE INDEX IF NOT EXISTS idx_zdb_tags_tag ON _zdb_tags(tag);

            CREATE TABLE IF NOT EXISTS _zdb_fields (
                zettel_id TEXT NOT NULL REFERENCES zettels(id),
                key TEXT NOT NULL,
                value TEXT,
                zone TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_zdb_fields_key ON _zdb_fields(key);

            CREATE TABLE IF NOT EXISTS _zdb_links (
                source_id TEXT NOT NULL REFERENCES zettels(id),
                target_path TEXT NOT NULL,
                display TEXT,
                zone TEXT,
                kind TEXT NOT NULL DEFAULT 'wikilink'
            );
            CREATE INDEX IF NOT EXISTS idx_zdb_links_target ON _zdb_links(target_path);

            CREATE TABLE IF NOT EXISTS _zdb_aliases (
                zettel_id TEXT NOT NULL REFERENCES zettels(id),
                alias TEXT COLLATE NOCASE NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_zdb_aliases_alias ON _zdb_aliases(alias);

            CREATE TABLE IF NOT EXISTS _zdb_meta (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS _zdb_fts USING fts5(
                title, body, tags,
                tokenize = 'porter unicode61'
            );",
        )?;

        Ok(Self { conn })
    }

    /// Upsert a single parsed zettel into the index.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn index_zettel(&self, zettel: &ParsedZettel) -> Result<()> {
        let id = zettel.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
```

Seven tables:
- **zettels** — core metadata (id, title, date, type, path, body)
- **_zdb_tags** — normalized tag-to-zettel mapping with `source` column (`frontmatter` or `body`)
- **_zdb_fields** — inline fields (key, value, zone) plus scalar frontmatter extras (String, Number, Bool) with `zone = 'Frontmatter'`
- **_zdb_links** — all link types (source, target, display text, zone, kind: `wikilink`/`embed`/`markdown`/`url`)
- **_zdb_aliases** — alternative names for zettels (title, aliases from fields)
- **_zdb_checkboxes** — checkbox items (state, content, date, due_date, line_number, indent_level) from body `- [ ]`/`[x]`/`[i]` syntax
- **_zdb_fts** — FTS5 virtual table (porter stemmer + unicode61 tokenizer)

WAL mode is enabled for concurrent read/write. The `_zdb_meta` table tracks the HEAD commit hash for staleness detection.

### Wikilink Resolution

`resolve_wikilink(target)` resolves a wikilink target string to a zettel path using a four-step fallback chain:

1. **Path lookup** — `SELECT ... FROM zettels WHERE path = target`. Handles path-qualified wikilinks like `zettelkasten/contact/20240619183742.md`.
2. **ID lookup** — `resolve_path(target)` matches against `zettels.id`. Handles bare ID wikilinks like `20240619183742`.
3. **Alias lookup** — `resolve_alias(target)` checks `_zdb_aliases` (case-insensitive). Handles human-readable aliases.
4. **Partial path matching** — `SELECT path FROM zettels WHERE path LIKE '%/' || target || '.md' OR path LIKE '%/' || target`. Matches filename stems (e.g., `meeting-notes` resolves to `zettelkasten/projects/meeting-notes.md`). When multiple matches exist, the shortest path wins.

Path-qualified links take precedence over ID-only links per the specification. Steps 1-3 are indexed lookups (O(1)). Step 4 uses a LIKE scan but is only reached as a last resort.

### Index Rebuild

`rebuild()` drops all tables (internal and materialized), recreates the schema from `SCHEMA_DDL`, then re-indexes every zettel from git. No migration framework — the index is a disposable cache, so schema changes are handled by rebuilding from scratch.

Rebuild uses a three-phase pipeline that separates I/O from CPU from SQL:

```text
rebuild()
  ├── list_zettels() → paths
  ├── parallel_parse(repo, paths)
  │     ├── read_files_batch(paths)          [sequential git I/O — optimal for pack files]
  │     └── contents.par_iter().map(parse)   [parallel CPU via rayon]
  │     → (Vec<ParsedZettel>, warnings)
  ├── batch_index(&parsed)                   [sequential SQL, single transaction]
  │     ├── BEGIN IMMEDIATE
  │     ├── for zettel in parsed: execute prepared stmts
  │     └── COMMIT
  ├── collect_consistency_warnings()
  └── materialize_all_types_from(&parsed)    [sequential SQL, no git reads]
        ├── infer_schema_from(type, &filtered)
        └── populate_from(schema, &filtered)
```

Key design decisions:

- **Sequential git reads**: `read_files_batch()` resolves HEAD tree once, iterates paths against the same tree. Git pack files benefit from sequential access patterns — parallelizing reads would thrash the pack.
- **Parallel parsing**: `par_iter()` (rayon) distributes CPU-bound `parser::parse()` across cores. `parser::parse` is thread-safe (uses `OnceLock` for compiled regexes).
- **Single transaction**: `batch_index()` wraps all SQL writes in one `BEGIN IMMEDIATE`/`COMMIT` instead of per-zettel savepoints, eliminating per-zettel fsync. WAL mode allows concurrent readers during the write.
- **Cached materialization**: `materialize_all_types_from()` reuses the `Vec<ParsedZettel>` from the parse phase. The old `materialize_all_types()` re-read every data zettel from git twice (once for schema inference, once for population). The `_from` variants filter the pre-parsed vector by type instead.
- **Error resilience**: Parse errors produce `ConsistencyWarning::MalformedYaml` warnings instead of aborting. One malformed zettel doesn't block the rest.

Full rebuild is only triggered by explicit `zdb reindex`, index corruption, or unreachable HEAD OID (e.g. after `git gc`). Normal operations use incremental reindex instead (see below).

The HEAD commit hash is stored in `_zdb_meta` so subsequent operations can detect when the index is stale (HEAD changed without reindex). `rebuild_if_stale()` checks this before reads.

### Incremental Reindex

When the stored HEAD exists and is reachable, `rebuild_if_stale()` uses `incremental_reindex` instead of a full rebuild. This diffs the old HEAD tree against the new HEAD tree using `git2::diff_tree_to_tree` (via `ZettelSource::diff_paths`) and only processes changed files:

- **Added/Modified**: read, parse, and upsert into SQLite
- **Deleted**: remove from the index
- **`_typedef` change**: triggers full `materialize_all_types` (schema changes affect table structure)

When multiple files are changed (2+), `incremental_reindex` uses `batch_index()` (single transaction) instead of per-zettel `index_zettel()`. Single-change diffs still use the per-zettel path to avoid transaction overhead.

If the diff fails (e.g. old HEAD was garbage-collected), it falls back to a full rebuild. At 1K zettels with 1 change, incremental reindex takes ~1ms vs ~200ms for a full rebuild.

### Full-Text Search

FTS5 with porter stemmer and unicode61 tokenizer enables natural language search. The primary entry point is `search_paginated`, which supports `LIMIT`/`OFFSET` pagination and returns a total count alongside results:

```rust
pub struct PaginatedSearchResult {
    pub hits: Vec<SearchResult>,
    pub total_count: usize,
}
```

The implementation runs two queries: the paginated FTS5 `MATCH` with `LIMIT ?2 OFFSET ?3`, and a `SELECT COUNT(*)` with the same match for total count. The convenience method `search()` delegates to `search_paginated(query, usize::MAX, 0)` and returns just the hits.

The FTS5 `MATCH` query handles boolean operators, phrase matching, and prefix queries. `snippet()` generates search result excerpts with `<b>` highlights. Results are ranked by BM25.

The GraphQL `search` field returns a `SearchConnection { hits, totalCount }` and accepts optional `limit` (default 20) and `offset` (default 0) arguments. The CLI `zdb search` exposes `--limit` and `--offset` flags with a "Showing X-Y of Z results" header.

## 10. SQL Engine — `zdb-core/src/sql_engine.rs`

The SQL engine translates standard SQL into zettel operations. This is what makes ZettelDB feel like a database — you can create zettel types with `CREATE TABLE` and manipulate them with `INSERT`, `UPDATE`, `DELETE`, and `SELECT`:

```bash
sed -n '1,35p' zdb-core/src/sql_engine.rs
```

```rust
use rusqlite::params;
use sqlparser::ast::{
    AlterTableOperation, AssignmentTarget, ColumnOption, DataType, Expr, FromTable, ObjectType,
    SetExpr, Statement, Value as SqlValue,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::collections::BTreeMap;

use crate::error::{Result, ZettelError};
use crate::indexer::Index;
use crate::parser;
use crate::traits::ZettelStore;
use crate::types::{
    ColumnDef, InlineField, Link, ParsedZettel, TableSchema, Value, ZettelId, ZettelMeta, Zone,
};

/// Strip surrounding double-quotes from a SQL identifier.
/// sqlparser preserves quotes in `to_string()` for identifiers like `"meeting-minutes"`.
fn unquote_identifier(s: &str) -> String {
    s.trim_matches('"').to_lowercase()
}

#[derive(Debug)]
pub enum SqlResult {
    Rows {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Affected(usize),
    Ok(String),
}
```

Two entry points:

- `execute(sql)` — single-statement convenience, delegates to `execute_batch`
- `execute_batch(sql)` — parses multiple semicolon-separated statements via `sqlparser`, executes sequentially

Statement dispatch in `execute_statement`:

```rust
match stmt {
    Statement::CreateTable(ct) => self.handle_create_table(ct),
    Statement::Insert(ins) => self.handle_insert(ins),
    Statement::Update { from: Some(_), .. } => /* reject UPDATE...FROM */,
    Statement::Update { .. } => self.handle_update(table, assignments, selection),
    Statement::Delete(del) => self.handle_delete(del),
    Statement::AlterTable { .. } => self.handle_alter_table(name, operations),
    Statement::Drop { object_type: Index|View, .. } => /* reject */,
    Statement::Drop { .. } => self.handle_drop(object_type, if_exists, names, cascade),
    Statement::CreateIndex(_) | CreateView { .. } | CreateVirtualTable { .. }
        | CreateTrigger { .. } | AlterIndex { .. } => /* reject */,
    Statement::StartTransaction { .. } => self.handle_begin(),
    Statement::Commit { .. } => self.handle_commit(),
    Statement::Rollback { .. } => self.handle_rollback(),
    _ => /* pass through to SQLite */,
}
```

The SQL engine uses `sqlparser` to parse SQL ASTs, then dispatches:

- **CREATE TABLE** → creates a `_typedef` zettel defining the schema, then materializes the corresponding SQLite table
- **INSERT** → creates one or more zettels of the table's type; supports multi-row `VALUES (...), (...)`; rejects `OR REPLACE`, `REPLACE INTO`, and `ON CONFLICT` modifiers (they bypass git)
- **UPDATE** → fast path for `WHERE id = '...'` (single zettel); bulk path delegates WHERE to SQLite via `resolve_matching_ids`, applies changes in batch; rejects `UPDATE...FROM` (ambiguous join-to-document mapping)
- **DELETE** → fast path for `WHERE id = '...'`; bulk path resolves matching rows via SQLite, deletes in batch via `delete_files`
- **ALTER TABLE** → ADD/DROP COLUMN modifies typedef and rematerializes; RENAME COLUMN rewrites typedef + all data zettels in a single commit
- **DROP TABLE** → without CASCADE strips `type:` from data zettels; with CASCADE deletes all; IF EXISTS is a no-op when missing
- **CREATE INDEX / VIEW / VIRTUAL TABLE / TRIGGER, ALTER INDEX, DROP INDEX / VIEW** → explicitly rejected with descriptive errors (these operate only on the materialized cache and would be lost on reindex)
- **SELECT / everything else** → passed through directly to SQLite

The key insight: SQL tables are zettel types. `CREATE TABLE project (...)` creates a `_typedef` zettel that defines the `project` type's schema. `INSERT INTO project VALUES (...)` creates a new zettel with `type: project` and the specified fields in its frontmatter/body/reference section.

**Folder namespaces**: Typedefs can declare `folder: true` in frontmatter to store instances in `zettelkasten/{type}/{id}.md` instead of flat `zettelkasten/{id}.md`. The `zettel_path()` helper in `git_ops.rs` computes the path based on type name and folder setting. Both `handle_insert()` in the SQL engine and `create_zettel()` in the actor/FFI use this helper. The `type_uses_folder()` method on `Index` looks up the typedef to check the folder setting. Types without `folder: true` (or without a typedef) remain flat for backward compatibility.

The bulk operations pattern uses `resolve_matching_ids` to delegate WHERE evaluation to SQLite — this reconstructs the WHERE clause via sqlparser's `Display` impl, runs `SELECT id FROM {table} WHERE {clause}` against the materialized table, then resolves each ID to a file path. This avoids reimplementing SQL expression evaluation in Rust.

### Quoted Identifier Handling

SQL identifiers with hyphens (like `meeting-minutes`) must be double-quoted in SQL. The `unquote_identifier` helper strips quotes for internal use:

```bash
sed -n '/^fn unquote_identifier/,/^}/p' zdb-core/src/sql_engine.rs
```

```rust
fn unquote_identifier(s: &str) -> String {
    s.trim_matches('"').to_lowercase()
}
```

All DDL/DML handlers (`handle_create_table`, `handle_insert`, `handle_update`, `handle_delete`, `handle_alter_table`, `handle_drop`) use `unquote_identifier` to normalize table names from the AST. Generated SQL statements quote table names with `"..."` to support hyphenated identifiers in SQLite.

### Multi-Row INSERT

`handle_insert` supports multi-row `VALUES (...), (...)`. The batch ID generator `unique_ids(count)` produces sequential timestamps without sleeping:

```bash
sed -n '/fn unique_ids/,/^    }/p' zdb-core/src/sql_engine.rs
```

```rust
    fn unique_ids(&mut self, count: usize) -> Result<Vec<ZettelId>> {
        use chrono::NaiveDateTime;

        let mut ids = Vec::with_capacity(count);
        let first = parser::generate_unique_id(|candidate| {
            self.index
                .conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM zettels WHERE id = ?1",
                    params![candidate],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        });

        let mut ts = NaiveDateTime::parse_from_str(&first.0, "%Y%m%d%H%M%S").map_err(|e| {
            ZettelError::SqlEngine(format!("failed to parse generated id timestamp: {e}"))
        })?;
        ids.push(first);

        for _ in 1..count {
            loop {
                ts += chrono::Duration::seconds(1);
                let candidate = ts.format("%Y%m%d%H%M%S").to_string();
                let exists: bool = self
                    .index
                    .conn
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM zettels WHERE id = ?1",
                        params![&candidate],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if !exists {
                    ids.push(ZettelId(candidate));
                    break;
                }
            }
        }

        Ok(ids)
    }
```

The first ID comes from `generate_unique_id` with an index-checking closure. Subsequent IDs increment by 1 second, skipping existing entries. After the per-row loop (validate, build zettel, index, materialize), all files commit in a single `commit_files` call. Returns comma-separated IDs for multi-row, single ID for single-row.

### Transactions

SqlEngine supports `BEGIN`/`COMMIT`/`ROLLBACK` to wrap multiple DML statements into a single git commit. All methods take `&mut self`.

When `BEGIN` executes, a SQLite `SAVEPOINT zdb_txn` is created and a `TransactionBuffer` initialized. DML within the transaction applies to SQLite immediately (enabling read-your-writes via SELECT) but buffers git operations as `PendingWrite`/`PendingDelete` entries. On `COMMIT`, all buffered writes and deletes flush to git in a single `commit_batch` call (commit message: `"transaction"`). On `ROLLBACK`, the SQLite savepoint rolls back and the buffer is discarded.

The `read_content` helper checks the transaction buffer before falling back to `repo.read_file`, enabling UPDATE of rows INSERTed within the same transaction. On COMMIT, cancelled operations (write then delete of the same path) are filtered out — this handles INSERT-then-DELETE within a transaction without touching git.

`Drop for SqlEngine` automatically rolls back any active transaction, preventing dangling savepoints on panic or early return. Nested `BEGIN` is rejected with an error.

The indexer methods (`index_zettel`, `remove_zettel`) use `SAVEPOINT`/`RELEASE` instead of `unchecked_transaction()` to nest correctly within the engine's savepoint.

## 11. Sync Manager — `zdb-core/src/sync_manager.rs`

The sync manager orchestrates multi-device sync with a cascade merge strategy:

```bash
sed -n '105,204p' zdb-core/src/sync_manager.rs
```

The sync flow (with per-phase timing instrumentation):

1. **Defer commit-graph** — `set_skip_commit_graph(true)` to avoid per-commit `git commit-graph write` overhead, with a drop guard that always restores `false` on sync exit (including error paths)
2. **Fetch** from remote
3. **Merge** — git's 3-way merge classifies the result
4. **Handle conflicts**:
   - **Delete-vs-edit**: edit wins, add `resurrected: true` to frontmatter, emit per-file `tracing::warn!` with `zettel_path` and `deleted_by` (local/remote)
   - **Normal conflicts**: run cascade resolve (CRDT → validate → LWW fallback)
   - **Commit merge** with both parents + HLC trailer
5. **Update sync state** — `known_heads = [HEAD]`, `last_sync = now`, commit `.nodes/{uuid}.toml`
6. **Push** — single push carries both merge result and node state
7. **Write commit-graph** once (re-enable with `set_skip_commit_graph(false)`)
8. **Incremental reindex** — `rebuild_if_stale()` diffs `old_head..new_head`, processes only changed files

This ordering (update state before push, single push) halves network round-trips. The incremental reindex reduced sync from ~10s to ~120ms at 5K zettels (see [Performance](sync.md#performance)).

### Cascade Merge Strategy

The `cascade_resolve()` method implements a three-step fallback:

```bash
sed -n '212,249p' zdb-core/src/sync_manager.rs
```

```rust
    fn cascade_resolve(
        &self,
        conflicts: Vec<ConflictFile>,
        strategy: Option<&str>,
    ) -> Vec<crate::types::ResolvedFile> {
        // Step 2: CRDT
        tracing::debug!(strategy = strategy.unwrap_or("preset:default"), "cascade_step2_crdt");
        match crdt_resolver::resolve_conflicts(conflicts.clone(), strategy) {
            Ok(resolved) => {
                // Validate each resolved file
                let all_valid = resolved.iter().all(|r| {
                    parser::parse(&r.content, &r.path).is_ok()
                });
                if all_valid {
                    return resolved;
                }
                tracing::warn!("CRDT resolution produced invalid output; falling back to LWW");
            }
            Err(e) => {
                tracing::warn!("CRDT resolution failed ({}); falling back to LWW", e);
            }
        }

        // Step 3: LWW by HLC
        match crdt_resolver::resolve_lww(conflicts.clone()) {
            Ok(resolved) => resolved,
            Err(_) => {
                // LWW should never fail, but if it does, ours-wins is the last resort
                conflicts
                    .into_iter()
                    .map(|c| crate::types::ResolvedFile {
                        path: c.path,
                        content: c.ours,
                    })
                    .collect()
            }
        }
    }
```

Three levels of fallback:
1. **Git merge** (already happened) — handles non-conflicting changes
2. **CRDT resolve** — per-zone Automerge merge. Validated by re-parsing the result.
3. **LWW by HLC** — if CRDT produces invalid output, pick the winner by timestamp
4. **Ours-wins** — absolute last resort if LWW somehow fails

The strategy for each zettel type is looked up from the `_typedef` zettel's `crdt_strategy` field, or falls back to the repo config's default.

## 12. Compaction — `zdb-core/src/compaction.rs`

Over time, CRDT temp files accumulate in `.crdt/temp/`. Compaction cleans them up and reports before/after storage measurements:

```bash
sed -n '249,305p' zdb-core/src/compaction.rs
```

```rust
/// Full compaction pipeline: threshold check → backup → shared head → cleanup → crdt doc compact → gc.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn compact(repo: &GitRepo, sync_mgr: &SyncManager, opts: &CompactOptions) -> Result<CompactionReport> {
```

`compact()` accepts `CompactOptions` (force, skip_backup, backup_path) instead of a bare `bool`. `default_backup_path()` computes the default location (`.zdb/backups/pre-compact-{ISO8601}.bundle.tar`), shared by both `backup_before_compact()` and the CLI dry-run path display. `backup_before_compact()` delegates to `bundle::export_full_bundle`. Backup runs after the threshold check but before any mutations, so a failed backup aborts compaction without side effects.

Compaction runs in stages, measuring storage before and after each phase:

1. **Threshold check** — skip if `.crdt/temp/` is under the configured MB limit (default 1MB). Below-threshold returns `backup_path: None`.
2. **Pre-compaction backup** — unless `skip_backup` is set, export a full Git bundle to `.zdb/backups/` (or a custom path). Uses `bundle::export_full_bundle` internally — the same mechanism as air-gapped sync. Backup failure aborts compaction.
3. **Measure before** — record CRDT temp bytes/files and `.git/` size
4. **Shared head** — compute the greatest common ancestor commit across all active nodes. CRDT files older than this are safe to delete because all nodes have seen them.
5. **Cleanup** — delete temp files at or before the shared head (both `.crdt` and `_fm.crdt`)
6. **Compact** — merge multiple Automerge documents for the same zettel into one. Body (`.crdt`) and frontmatter (`_fm.crdt`) files are grouped and compacted independently.
7. **Measure after** — record CRDT temp bytes/files post-compaction
8. **Git GC** — run `git gc` to reclaim space, then measure `.git/` again

Stale and retired nodes are excluded from the shared head calculation, so an offline device doesn't block compaction forever.

The `CompactionReport` returned includes `backup_path`, `crdt_temp_bytes_before/after`, `crdt_temp_files_before/after`, and `repo_bytes_before/after` — giving operators full visibility into what compaction achieved. See [Storage Budget](storage-budget.md) for measured growth with and without compaction (97% reduction at 10 edits/day).

When a stale node returns after compaction has removed CRDT history, the conflict resolution cascade still works: it reconstructs three-way merges from Git content, falling back to LWW if CRDT merge produces invalid output. See [Conflict Resolution Cascade](sync.md#conflict-resolution-cascade) for the full decision tree.

Recovery from a pre-compaction backup: `zdb bundle import <backup.bundle.tar>` on a fresh `zdb init` restores the full repository state as of the backup moment. See [Bundle](bundle.md) for format details.

That threshold behavior matters for tests and smoke scripts: a plain `zdb compact` on a tiny repo is allowed to omit the backup line entirely because no compaction work ran. The smoke coverage that asserts `backup:` now uses `zdb compact --force`, while the non-forced path is covered separately by the dry-run and under-threshold tests.

## 12a. Git Maintenance — `zdb-core/src/maintenance.rs`

The maintenance module delegates git repository housekeeping (commit-graph updates, incremental repacks, loose object cleanup) to `git maintenance run --auto`. It complements compaction — compaction handles CRDT temp files, maintenance handles git-level storage optimization.

### Probe and Fallback

`probe_git_maintenance()` checks whether the installed `git` supports the `maintenance` subcommand (added in git 2.29). The result is cached per process in an `OnceLock<bool>`. If unavailable, all maintenance calls fall back to `git gc --auto`.

### Public API

```rust
pub fn run(repo_path: &Path, tasks: Option<&[&str]>) -> Result<MaintenanceReport>;
pub fn maybe_auto_run(repo: &GitRepo);
pub fn check_write_threshold(repo: &GitRepo);
```

`run()` shells out to `git maintenance run --auto` (or `--task=<t>` for explicit task selection). Returns a `MaintenanceReport` with `success`, `duration_ms`, `fallback_used`, and `tasks_run`. Errors from git are logged but still returned — callers decide whether to propagate.

`maybe_auto_run()` is the automatic trigger for sync/compact. It loads `RepoConfig`, checks `maintenance.auto_enabled`, and calls `run()` if enabled. All errors are caught and logged internally — the function never propagates failures to callers, so `compact()` and `sync()` can call it unconditionally.

`check_write_threshold()` is the high-write session trigger. It increments a per-repo atomic commit counter (`GitRepo::session_commits`) and checks if it has crossed `MaintenanceConfig::write_threshold` (default: 50). When the threshold is reached, it runs maintenance and resets the counter. Called automatically from `commit_files()` and `commit_binary_file()`.

### Configuration

Auto-maintenance is configured in `.zetteldb.toml`:

```toml
[maintenance]
auto_enabled = false   # default: opt-in
write_threshold = 50   # commits before auto-maintenance triggers
```

The `MaintenanceConfig` struct uses `#[serde(default)]` so existing config files without a `[maintenance]` section load cleanly.

### Integration Points

- **Compaction** — `maybe_auto_run()` is called at the end of `compact()`, after git gc
- **Sync** — `maybe_auto_run()` is called at the end of `SyncManager::sync()`, before the final timing log
- **High-write sessions** — `check_write_threshold()` is called from `commit_files()` and `commit_binary_file()` after every successful commit
- **CLI** — `zdb maintenance run [--task <t>]` for explicit runs; `zdb maintenance auto on|off|status` to toggle
- **Server** — `maintenance(task: String)` GraphQL mutation via the actor
- **FFI** — `ZettelDriver::run_maintenance()` returns success bool

### Platform Behavior

| Environment | Behavior |
|---|---|
| macOS/Linux with git >= 2.29 | `git maintenance run --auto` |
| Older git | Falls back to `git gc --auto`, `fallback_used: true` |
| No git binary (mobile/FFI) | `run()` returns `Err(Io(...))` |

## 13. CLI — `zdb-cli/src/main.rs`

The `zdb` binary is a thin shell over `zdb-core`. Clap provides the command structure. Subcommands are divided into **stable** and **experimental** tiers — experimental commands show an `[experimental]` prefix in `--help` output. See the Stability Tiers section below for the full classification.

```bash
sed -n '11,125p' zdb-cli/src/main.rs
```

```rust
#[derive(Parser)]
#[command(name = "zdb", about = "Decentralized Zettelkasten")]
struct Cli {
    /// Repository path (default: current directory)
    #[arg(short, long, default_value = ".")]
    repo: PathBuf,

    /// Directory for NDJSON log files (default: stderr with env filter)
    #[arg(long, global = true, env = "ZDB_LOG_DIR")]
    log_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new zettelkasten repository
    Init {
        /// Path to create the repository
        path: Option<PathBuf>,
    },
    /// Create a new zettel
    Create {
        #[arg(long)]
        title: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        #[arg(long)]
        body: Option<String>,
    },
    /// Read a zettel by ID
    Read {
        /// Zettel ID
        id: String,
    },
    /// Update an existing zettel
    Update {
        /// Zettel ID
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        #[arg(long)]
        body: Option<String>,
    },
    /// Sync with remote
    Sync {
        /// Remote name
        #[arg(default_value = "origin")]
        remote: String,
        /// Branch name
        #[arg(default_value = "master")]
        branch: String,
    },
    /// Execute SQL (DDL/DML routed through SQL engine; SELECT queries index)
    Query {
        /// SQL statement
        sql: String,
    },
    /// Full-text search
    Search {
        /// Search query
        query: String,
        #[arg(long, default_value = "20")]
        limit: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Register this device as a sync node
    RegisterNode {
        /// Device name
        name: String,
    },
    /// Show repository status
    Status,
    /// Compact CRDT history and run git gc
    Compact {
        /// Force compaction even if under threshold
        #[arg(long)]
        force: bool,
        /// Show what would be done without doing it
        #[arg(long)]
        dry_run: bool,
        /// Skip pre-compaction backup bundle
        #[arg(long)]
        no_backup: bool,
        /// Custom path for backup bundle
        #[arg(long)]
        backup_path: Option<PathBuf>,
    },
    /// Rebuild the search index
    Reindex,
    /// Type definition management
    Type {
        #[command(subcommand)]
        action: TypeAction,
    },
    /// Node management
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// Export/import bundles for air-gapped sync
    Bundle {
        #[command(subcommand)]
        action: BundleAction,
    },
    /// Start GraphQL API server
    Serve {
        #[arg(long, default_value = "2891")]
        port: u16,
        #[arg(long, default_value = "2892")]
        pg_port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long)]
        playground: bool,
    },
}
```

Each command follows the same pattern: open repo → open index → perform operation → print result. The CLI delegates directly to `zdb-core` functions, adding only I/O formatting.


### Pipe-Safe Stdout

Pipe-oriented commands such as `zdb type suggest foo | grep -q bar` and `zdb register-node Laptop | head -n 1` used to panic when the downstream process closed stdout early. The CLI now routes normal stdout through fallible helpers so a broken pipe returns cleanly instead of surfacing as a Rust panic.

```bash
sed -n '1,41p' zdb-cli/src/main.rs
```

```rust
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use zdb_core::compaction;
use zdb_core::git_ops::{self, GitRepo};
use zdb_core::indexer::Index;
use zdb_core::parser;
use zdb_core::sql_engine::{SqlEngine, SqlResult};
use zdb_core::sync_manager::{self, SyncManager};

mod updater;

macro_rules! out {
    ($($arg:tt)*) => {
        write_stdout(format_args!($($arg)*))
    };
}

macro_rules! outln {
    ($($arg:tt)*) => {
        writeln_stdout(format_args!($($arg)*))
    };
}

fn write_stdout(args: std::fmt::Arguments<'_>) -> zdb_core::error::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.flush()?;
    Ok(())
}

fn writeln_stdout(args: std::fmt::Arguments<'_>) -> zdb_core::error::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn is_broken_pipe(err: &zdb_core::error::ZettelError) -> bool {
```

```bash
sed -n '291,297p' zdb-cli/src/main.rs
```

```rust
    if let Err(e) = run(cli) {
        if is_broken_pipe(&e) {
            return;
        }
        eprintln!("error: {e}");
        std::process::exit(1);
    }
```

The `Serve` command bootstraps a tokio runtime and launches the async server. Logging supports two modes: NDJSON to a file (for structured logging) or stderr with `RUST_LOG` env filter (for development).

### Self-Update — `zdb-cli/src/updater.rs`

The updater provides zero-friction binary updates from pre-built releases at `github.com/doogat/zdb`.

**Auto-update flow (every command):** `main()` calls `notify_if_updated()` which reads the state file at `~/.config/zetteldb/update-check.json`. If `updated_from` is set (meaning a background auto-update replaced the binary), it prints a one-line notice and clears the flag. Then `spawn_background_check()` fires if >1h since last check — it re-execs `zdb __update-check` as a detached process that hits the GitHub releases API, and if a newer version exists, downloads + verifies + replaces the binary automatically. Zero latency impact on the actual command.

**Explicit flow (`zdb update-bin`):** Synchronous fetch of latest release → semver compare → download `.tar.gz` archive → verify SHA-256 checksum → extract binary → verify `--version` output → `self_replace::self_replace()`. The `UpdateBin` variant is handled in `main()` before `run()` to avoid requiring a `--repo` path.

The `__update-check` hidden subcommand exists solely as the background auto-update entry point.

## 14. Server — `zdb-server/`

> **Experimental**: The entire server crate is experimental. All HTTP responses include an `X-Experimental: true` header via an axum `map_response` middleware layer applied after auth.

The server exposes ZettelDB through three protocols: HTTP/GraphQL, WebSocket (for real-time events), and PostgreSQL wire protocol (pgwire).

### Actor Pattern

Since `GitRepo` and `Index` are not `Send` (they use `git2::Repository` and `rusqlite::Connection`), the server uses an actor pattern: a single background OS thread owns the repo and index, accepting commands via an mpsc channel:

```bash
sed -n '17,97p' zdb-server/src/actor.rs
```

```rust
/// Commands the actor understands.
pub enum ActorCommand {
    GetZettel {
        id: String,
    },
    ListZettels {
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    },
    Search {
        query: String,
        limit: usize,
        offset: usize,
    },
    CreateZettel {
        title: String,
        body: Option<String>,
        tags: Vec<String>,
        zettel_type: Option<String>,
    },
    UpdateZettel {
        id: String,
        title: Option<String>,
        body: Option<String>,
        tags: Option<Vec<String>>,
        zettel_type: Option<String>,
    },
    DeleteZettel {
        id: String,
    },
    ExecuteSql {
        sql: String,
    },
    GetTypeSchemas,
    GetBacklinks {
        id: String,
    },
    CountZettels {
        zettel_type: Option<String>,
        tag: Option<String>,
        backlinks_of: Option<String>,
    },
    FilteredList {
        table_name: String,
        where_sql: String,
        params: Vec<rusqlite::types::Value>,
        order_sql: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    },
    AggregateQuery {
        sql: String,
        params: Vec<rusqlite::types::Value>,
    },
    RunMaintenance { force: bool },
    Sync { remote: String, branch: String },
    NoSqlGet { id: String },
    NoSqlScanType { type_name: String },
    NoSqlScanTag { tag: String },
    NoSqlBacklinks { id: String },
}

/// Replies from the actor.
pub enum ActorReply {
    Zettel(Box<ActorResult<ParsedZettel>>),
    ZettelList(ActorResult<Vec<ParsedZettel>>),
    SearchResults(ActorResult<PaginatedSearchResult>),
    SqlResult(ActorResult<SqlResult>),
    TypeSchemas(ActorResult<Vec<TableSchema>>),
    Backlinks(ActorResult<Vec<String>>),
    Deleted(ActorResult<()>),
    Count(ActorResult<i64>),
    AggregateRow(ActorResult<Vec<String>>),
    Maintenance(ActorResult<CompactionReport>),
    SyncResult(ActorResult<SyncReport>),
    NoSqlZettel(Box<ActorResult<Option<ParsedZettel>>>),
    NoSqlIds(ActorResult<Vec<String>>),
}

struct ActorMsg {
    cmd: ActorCommand,
    reply: oneshot::Sender<ActorReply>,
}

/// Async handle to the repo actor.
#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::Sender<ActorMsg>,
    event_bus: EventBus,
}

impl ActorHandle {
    /// Spawn the actor on a std::thread. Returns the handle for async callers.
    pub fn spawn(repo_path: PathBuf, event_bus: EventBus) -> ActorResult<Self> {
        // Validate repo opens before spawning
        let _ = GitRepo::open(&repo_path)?;

        let (tx, rx) = mpsc::channel::<ActorMsg>(64);
        let bus = event_bus.clone();
        std::thread::spawn(move || {
            actor_loop(repo_path, rx, bus);
        });
        Ok(Self { tx, event_bus })
```

`ActorHandle` is `Clone + Send + Sync` — it's just an `mpsc::Sender`. Async route handlers send commands through the channel and `await` a oneshot reply. The actor thread runs a blocking loop (`rx.blocking_recv()`) processing one command at a time — no locks needed since it's single-threaded.

Mutations (create, update, delete) emit events through an `EventBus` for WebSocket subscribers. When the `nosql` feature is active, the actor also dual-writes to `RedbIndex` after each mutation for O(1) key-value access.

#### ReadPool — Concurrent Read Path

Read operations bypass the actor entirely and execute through `ReadPool` (`zdb-server/src/read_pool.rs`). Each read acquires a semaphore permit, then runs in `tokio::task::spawn_blocking` with freshly-opened `Index` and `GitRepo` handles:

```rust
async fn with_index_repo<F, T>(&self, f: F) -> Result<T>
where
    F: FnOnce(&Index, &GitRepo) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let permit = self.acquire().await?;
    let inner = self.inner.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let index = Index::open(&inner.db_path)?;
        let repo = GitRepo::open(&inner.repo_path)?;
        f(&index, &repo)
    })
    .await
    .map_err(|e| ZettelError::Validation(format!("read task panicked: {e}")))?
}
```

Key design decisions:
- `git2::Repository` is `!Send + !Sync` — fresh handles must be opened inside the blocking closure
- `OwnedSemaphorePermit` (via `acquire_owned`) is moved into the closure for correct cancellation semantics
- Three dispatch helpers: `with_index` (index-only), `with_index_repo` (index + git), `with_redb` (NoSQL)
- SQLite WAL mode allows concurrent readers without blocking the writer actor
- Default pool size: `min(available_parallelism, 4)`, configurable via `config.toml [server.read_pool_size]`, clamped to minimum 1 to prevent deadlock
- `FilteredListQuery` struct bundles the 7 parameters for `filtered_list()` calls (table, where, params, order, tag, limit, offset)

The `sql` query field and pgwire `do_query` use `sqlparser` to classify queries: pure `SELECT` statements (single-statement, no INSERT...SELECT or CREATE TABLE AS SELECT) route through `ReadPool.execute_select()`, everything else goes to the actor.

#### Sync & Compact Commands

Two operational commands extend the actor beyond CRUD. `Sync` triggers a full sync cycle, and `RunMaintenance` runs compaction — both return structured reports to the GraphQL layer.

`run_sync` creates a per-call SyncManager, calls `sync()`, then rebuilds the index:

```bash
grep -A7 '^fn run_sync' zdb-server/src/actor.rs
```

```rust
fn run_sync(repo: &GitRepo, index: &Index, remote: &str, branch: &str) -> ActorResult<SyncReport> {
    let mut mgr = zdb_core::sync_manager::SyncManager::open(repo)?;
    let report = mgr.sync(remote, branch, index)?;
    // Rebuild index after sync
    index.rebuild_if_stale(repo)?;
    Ok(report)
}
```

`run_maintenance` returns a no-op `CompactionReport` when no node is registered (matching the old behavior of returning `Ok(())`). It builds `CompactOptions` from the `force`, `no_backup`, and `backup_path` parameters. The GraphQL `compact` mutation exposes all three as input args (`force: Boolean`, `noBackup: Boolean`, `backupPath: String`) and returns `CompactResult` (including nullable `backupPath`). The `sync` mutation exposes `SyncReport` fields as `SyncResult`.

### Server Startup

```bash
cat zdb-server/src/lib.rs
```

```rust
pub mod actor;
pub mod auth;
pub mod config;
pub mod error;
pub mod events;
pub mod filter;
pub mod maintenance;
pub mod nosql_api;
pub mod pgwire;
pub mod read_pool;
pub mod reload;
pub mod rest;
pub mod schema;
pub mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::{middleware, Extension, Router};
use async_graphql::dynamic::Schema;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};

use actor::ActorHandle;
use auth::AuthToken;
use config::ServerConfig;
use events::EventBus;

/// Run the GraphQL server.
pub async fn run(
    repo_path: PathBuf,
    port: Option<u16>,
    pg_port: Option<u16>,
    bind: Option<&str>,
    playground: bool,
) -> std::io::Result<()> {
    let cfg = ServerConfig::load(port, pg_port, bind);

    // Auth
    let token = auth::load_or_create_token(&cfg.token_file)?;
    eprintln!("auth token: {}", cfg.token_file.display());

    // Actor
    let event_bus = EventBus::new();
    let actor = ActorHandle::spawn(repo_path.clone(), event_bus).map_err(|e| {
        std::io::Error::other(e.to_string())
    })?;

    // Read pool for concurrent read-only queries
    let read_pool = read_pool::ReadPool::new(repo_path, cfg.read_pool_size)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // Fetch type schemas for dynamic schema generation
    let type_schemas = actor.get_type_schemas().await.unwrap_or_default();
    let type_count = type_schemas.len();

    // Build GraphQL schema with hot-reload support (two-phase init)
    let rest_actor = actor.clone();
    let (reloader, shared_schema) = reload::SchemaReloader::new(actor.clone(), read_pool.clone());
    let gql_schema = match schema::build_schema(actor, read_pool.clone(), type_schemas, Some(reloader.clone())) {
        Ok(s) => s,
        Err(e) => {
            log::error!("failed to build initial GraphQL schema: {e}");
            return Err(std::io::Error::other(e));
        }
    };
    reloader.store_initial(gql_schema);

    // Auth-gated routes
    let mut auth_routes = Router::new()
        .route("/graphql", axum::routing::post(graphql_handler))
        .nest("/rest", rest::router())
        .nest("/nosql", nosql_api::router());

    if playground {
        // ... playground route on /graphql GET ...
    }

    let auth_routes = auth_routes.layer(middleware::from_fn(auth::require_auth));

    // WebSocket route — auth handled in ws_handler via header or connection_init payload
    let ws_routes = Router::new().route("/ws", axum::routing::get(ws::ws_handler));

    let pg_actor = rest_actor.clone();
    let pg_token = token.clone();
    let pg_reloader = reloader.clone();
    let pg_read_pool = read_pool.clone();

    // Merge routers — shared extensions available to all routes
    let app = auth_routes
        .merge(ws_routes)
        .layer(Extension(AuthToken(token)))
        .layer(Extension(rest_actor))
        .layer(Extension(read_pool))
        .layer(Extension(shared_schema));

    let addr = format!("{}:{}", cfg.bind, cfg.port);
    eprintln!("listening on {addr}");
    eprintln!("{type_count} type schema(s) loaded");

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    let pg = pgwire::start(
        pg_actor,
        pg_read_pool,
        pg_token,
        pg_reloader,
        &cfg.bind,
        cfg.pg_port,
    );

    tokio::select! {
        r = axum::serve(listener, app) => r?,
        r = pg => r?,
    };
    Ok(())
}

async fn graphql_handler(
    Extension(schema): Extension<Arc<ArcSwap<Schema>>>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let schema = schema.load();
    schema.execute(req.into_inner()).await.into()
}
```

Startup sequence:

1. Load config from `~/.config/zetteldb/`
2. Generate or load bearer auth token
3. Spawn the actor thread with an EventBus (actor opens `RedbIndex` and rebuilds it at startup)
4. Create `ReadPool` (semaphore-gated, `spawn_blocking` dispatch with fresh Index + GitRepo per request)
5. Fetch `_typedef` schemas and build a dynamic GraphQL schema (`build_schema() -> Result<Schema, String>` — aborts startup on failure)
6. Set up `SchemaReloader` backed by `arc_swap::ArcSwap` for hot-reload when types change
7. Spawn background maintenance task if `maintenance_enabled` (default: on, interval: 1h)
8. Mount routes: `/graphql` (POST), `/ws` (WebSocket), `/rest/*` (REST API), `/nosql/*` (NoSQL key-value)
   - REST list (`GET /rest/zettels`) uses `Query<HashMap<String, String>>` for dynamic `field.{name}=value` params alongside standard filters (`type`, `tag`, `q`, `backlinks`). Field filters pass through to `build_filtered_sql` as `IN (SELECT zettel_id FROM _zdb_fields ...)` conditions.
9. Start pgwire on a separate port (default 2892), with both `ActorHandle` (writes) and `ReadPool` (SELECTs)
10. `tokio::select!` runs both HTTP and pgwire concurrently

The GraphQL schema is **dynamic** — built at runtime from `_typedef` zettels. If a user creates a `project` type with fields `status` and `deadline`, the GraphQL schema gains a `project` query type with those fields. The `SchemaReloader` uses `ArcSwap` so the schema can be atomically replaced without restarting the server.

**Checkbox queries**: The schema includes `checkboxItems(state, zettelId, limit, offset)` and `openActions(limit)` queries. These route through `ReadPool::query_checkboxes()` which joins `_zdb_checkboxes` with `zettels` to return items with parent zettel context. `openActions` is a convenience alias that filters for `state = 'open'`.

The reload loop (`SchemaReloader::reload_loop`) handles errors gracefully: if `get_type_schemas()` fails or `build_schema()` returns `Err`, the error is logged and the last-known-good schema is preserved. Invalid typedef table names (those failing `is_valid_graphql_name()`) are skipped with a warning during schema build, so one bad typedef doesn't poison the entire schema.

Auth is bearer-token based. The token file lives at `~/.config/zetteldb/token`. All routes except `/ws` pass through the `require_auth` middleware. The `/ws` route handles auth in the handler itself (see below).

### WebSocket Authentication

The `/ws` route is split from the auth-gated router so browser clients can connect without setting HTTP headers. `ws_handler` in `ws.rs` implements dual-path auth:

```rust
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    protocol: GraphQLProtocol,
    Extension(shared_schema): Extension<Arc<ArcSwap<Schema>>>,
    Extension(auth): Extension<AuthToken>,
    headers: axum::http::HeaderMap,
) -> Response
```

1. **Header present + valid** (`validate_bearer` from `auth.rs`): session pre-authenticated, `on_connection_init` is a no-op that returns `Ok(Data::default())`
2. **Header absent**: `on_connection_init` callback extracts `payload["Authorization"]`, validates via `validate_bearer`. Valid → `connection_ack`; missing → `"Missing authentication"` error; invalid → `"Invalid authentication"` error
3. **Header present + invalid**: returns `StatusCode::UNAUTHORIZED` before upgrade

The `validate_bearer` helper (extracted from `require_auth`) checks `"Bearer "` prefix and compares the token. Both the middleware and `ws_handler` use it.

### Filtering, Sorting, and Aggregation

The `filter.rs` module (883 lines) generates all the GraphQL types and SQL builders needed for rich per-type queries. It's structured in layers — shared scalar filters → per-type input generation → SQL compilation.

**Shared scalar filter types** define five reusable input objects: `StringFilter`, `IntFilter`, `FloatFilter`, `BoolFilter`, and `IDFilter`. Each exposes operator fields appropriate to its type:

```bash
sed -n '10,56p' zdb-server/src/filter.rs
```

```rust
pub fn string_filter() -> InputObject {
    InputObject::new("StringFilter")
        .field(InputValue::new("eq", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("neq", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("contains", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("startsWith", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::STRING)))
}
// ... int_filter(), float_filter(), bool_filter(), id_filter() follow the same pattern
```

**Per-type Where inputs** are generated from `TableSchema` columns. `build_where_input` maps each column to the matching scalar filter type and adds `_and`/`_or` fields for compound logic:

```bash
sed -n '75,89p' zdb-server/src/filter.rs
```

```rust
pub fn build_where_input(type_name: &str, schema: &TableSchema) -> InputObject {
    let name = format!("{type_name}Where");
    let mut input = InputObject::new(&name);
    for col in &schema.columns {
        let filter_type = filter_type_for_column(col);
        input = input.field(InputValue::new(&col.name, TypeRef::named(filter_type)));
    }
    input = input.field(InputValue::new("_and", TypeRef::named_list(&name)));
    input = input.field(InputValue::new("_or", TypeRef::named_list(&name)));
    input
}
```

**WHERE clause compilation** in `build_where_sql` walks the GraphQL filter object recursively, emitting parameterized SQL. Column names are validated against the schema to prevent injection. `_and`/`_or` combinators produce nested `(... AND ...)` / `(... OR ...)` groups:

```bash
sed -n '334,409p' zdb-server/src/filter.rs
```

```rust
pub fn build_where_sql(input: &GqlValue, schema: &TableSchema) -> WhereClause {
    let mut conditions = Vec::new();
    let mut params = Vec::new();
    let obj = match input {
        GqlValue::Object(obj) => obj,
        _ => return WhereClause::empty(),
    };
    for (name, value) in obj {
        match name.as_str() {
            "_and" => { /* recursively AND sub-clauses */ }
            "_or"  => { /* recursively OR sub-clauses */ }
            field  => {
                // Validate column exists, then build operator conditions
                if schema.columns.iter().any(|c| c.name == field) {
                    // each operator (eq, gt, contains...) adds "col OP ?" + param
                }
            }
        }
    }
    WhereClause { sql: conditions.join(" AND "), params }
}
```

All filter values become `?` placeholders with corresponding `rusqlite::types::Value` entries — no string interpolation touches user input.

**Sorting**: `build_order_by_input` generates a `{Type}OrderBy` input where each column maps to the `SortOrder` enum (`ASC`/`DESC`). `build_order_sql` compiles this into an ORDER BY clause, validating column names against the schema.

**Connection wrapper**: `build_connection_type` generates `{Type}Connection` with `items: [Type!]!` and `totalCount: Int!`. The resolver extracts these from a `GqlValue::Object` passed by the parent query. `totalCount` reflects total matching rows (ignoring `limit`/`offset`) for pagination.

**Aggregation**: `build_aggregate_type` generates `{Type}Aggregate` with `count: Int!` plus `min{Col}`/`max{Col}`/`sum{Col}`/`avg{Col}` as nullable Float fields for each numeric column. `build_aggregate_sql` compiles this into `SELECT COUNT(*) AS count, MIN("col"), ... FROM "table"` with optional WHERE clause.

The actor processes these through two new commands: `FilteredList` (runs parameterized `SELECT id FROM "{table}" WHERE ...` then hydrates zettels from git) and `AggregateQuery` (runs raw aggregate SQL, returns a single row of string values).

## 15. Test Coverage

The codebase has comprehensive tests at every level:

```bash
grep -c '#\[test\]' zdb-core/src/*.rs zdb-core/src/**/*.rs zdb-server/src/*.rs 2>/dev/null | grep -v ':0$'
```

```rust
zdb-core/src/bundle.rs:3
zdb-core/src/bundled_types.rs:7
zdb-core/src/compaction.rs:9
zdb-core/src/crdt_resolver.rs:25
zdb-core/src/git_ops.rs:19
zdb-core/src/hlc.rs:10
zdb-core/src/indexer.rs:33
zdb-core/src/nosql.rs:3
zdb-core/src/parser.rs:31
zdb-core/src/sql_engine.rs:15
zdb-core/src/sync_manager.rs:10
zdb-server/src/filter.rs:23
zdb-server/src/pgwire.rs:2
```

```bash
wc -l tests/smoke.sh
```

```rust
382 tests/smoke.sh
```

```bash
head -30 tests/smoke.sh
```

```rust
#!/usr/bin/env bash
set -euo pipefail

# Build and lint
cargo clippy --workspace --quiet
cargo build --quiet
cargo bench --no-run --quiet 2>/dev/null
ZDB="$(cargo metadata --format-version=1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug/zdb"

# Work in temp directories, clean up on exit
TMPDIR="$(mktemp -d)"
REMOTE_DIR="$(mktemp -d)"
NODE1_DIR="$(mktemp -d)"
NODE2_DIR="$(mktemp -d)"
NODE3_DIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR" "$REMOTE_DIR" "$NODE1_DIR" "$NODE2_DIR" "$NODE3_DIR"' EXIT
cd "$TMPDIR"

pass() { printf '  ✓ %s\n' "$1"; }

echo "=== smoke test ==="

pass "clippy + bench compile"

# 1. init
$ZDB init . >/dev/null
pass "init"

# 2. create zettels (no sleeps — tests cross-process ID uniqueness)
ID1=$($ZDB create --title "First note" --tags "test,smoke" --body "Hello world")
ID2=$($ZDB create --title "Links to first" --body "See [[$ID1]]")
```

167 unit tests across 12 modules, plus a smoke harness with two profiles. `SMOKE_PROFILE=quick` stops after the single-repo CLI/SQL/compact path and is meant for fast local and CI checks. The default `full` profile continues through server protocols (GraphQL, REST, PgWire, NoSQL), multi-node sync with conflict resolution, and air-gapped bundle sync.

### Property-Based Tests

`zdb-core/tests/property_tests.rs` uses proptest to systematically explore input spaces. 51 property tests across four subsystems:

**Parser (20 tests, small local smoke budgets):** parse-serialize-parse idempotency, zone isolation (body edits don't affect frontmatter/references), no false reference boundary detection from thematic breaks, malformed frontmatter never panics (YAML-special chars, random bytes, truncated delimiters, deeply nested YAML, mixed valid/invalid fields), full Unicode round-trips for titles/tags/body/extra fields, empty/missing zone combinations, boundary cases (10K-char lines, 100+ tags, 500+ references, 50+ extra fields).

**SQL Engine (15 tests, very small local smoke budgets):** valid DDL/DML round-trips (CREATE TABLE, INSERT, SELECT, UPDATE, DELETE, typedef verification), invalid input never panics (random strings, partial fragments, unsupported operations, empty input), injection safety (embedded quotes/semicolons/comments stored as data not executed), edge-case identifiers (reserved words, hyphens, Unicode in double-quoted identifiers), structural edge cases (multiple semicolons, nested parentheses).

**CRDT Resolver (7 tests, small local smoke budgets):** merge commutativity for non-overlapping frontmatter, body, and reference edits; frontmatter field independence (changing X doesn't affect Y); merge idempotency (identical ours/theirs → original); non-overlapping body edits both survive; concurrent reference additions produce union.

**Indexer (9 tests, small local smoke budgets):** sequential `index_zettel` calls produce same query results as full `rebuild`, staleness detection correct after commit change, FTS5 query fuzzing (random strings, FTS5 operators, long queries, empty queries never crash), type inference consistency (same-type fields preserved), deterministic indexing (two DBs with same input match), mixed-type widening never panics.

Run with `cargo test -p zdb-core --test property_tests`. Override case count via `PROPTEST_CASES` env var (e.g. 50 for CI).

One subtle failure mode showed up only at high case counts on shared CI runners: the Unicode round-trip properties originally built strings from `any::<char>()` and then filtered out YAML-hostile characters with a long deny-list. That worked functionally, but at 5K cases proptest could burn through its local reject budget before it found enough acceptable strings, aborting with `Too many local rejects` after spending hours in the test job. The generator now samples directly from a curated set of YAML-safe Unicode characters across multiple scripts and normalizes whitespace, so the Unicode properties still exercise non-ASCII serialization paths without reject storms.

The local default case counts are intentionally tiny smoke budgets now. That keeps `cargo test` usable during development while still replaying any saved regressions. Two implementation details matter for runtime: the SQL/indexer property tests use `Index::open_in_memory()` instead of creating a fresh on-disk SQLite database for every case, and the default case counts are much lower for SQL/CRDT/indexer blocks than for parser-only blocks. The workspace root now reinforces that split by making plain `cargo test` target only `zdb-core`, `zdb-cli`, and `zdb-server`; `cargo test-full` / `cargo test --workspace` restores the full workspace including `zdb-e2e`. CI goes one step further by keeping `cargo test-ci` to unit/bin targets in the OS matrix and running `property_tests` / `sync_test` once on Linux, while `PROPTEST_CASES=5000 cargo test -p zdb-core --test property_tests -- --nocapture` remains the manual soak command.

## Data Flow Summary

Here's how the pieces connect for the most common operations:

**Create a zettel:**

```
CLI/GraphQL → parser::serialize(ParsedZettel) → git_ops::commit_file → indexer::index_zettel
```

**Read a zettel:**

```
CLI/GraphQL → indexer::resolve_path(id) → git_ops::read_file → parser::parse → ParsedZettel
```

**Search:**

```
CLI/GraphQL → indexer::search_paginated(query, limit, offset) → FTS5 MATCH + COUNT(*) → PaginatedSearchResult { hits, total_count }
```

**SQL query:**

```
CLI/GraphQL → sql_engine::execute(sql) → sqlparser AST → dispatch:
  SELECT → indexer (SQLite passthrough)
  INSERT → parser::serialize → git_ops::commit → indexer::index
  CREATE TABLE → build _typedef zettel → git_ops::commit → indexer::materialize
```

**Sync:**

```
CLI → sync_manager::sync:
  set_skip_commit_graph(true) + drop guard reset
  → git_ops::fetch → git_ops::merge_remote → MergeResult::Conflicts?
    → crdt_resolver::resolve_conflicts (per-zone Automerge)
    → validate → fallback to LWW if invalid
    → git_ops::commit_merge
  → update_sync_state → git_ops::push (single)
  → write_commit_graph (once) → indexer::rebuild_if_stale (incremental)
```

**Incremental reindex:**

```
rebuild_if_stale → stored_head_oid (from _zdb_meta) → diff_paths(old_head, new_head)
  → Added/Modified: read + parse + index_zettel (upsert)
  → Deleted: remove_zettel
  → _typedef changed? → materialize_all_types
  → diff error? → fallback to full rebuild
```

**Compaction:**

```
CLI → compaction::compact(CompactOptions):
  threshold_check → backup_before_compact(bundle::export_full_bundle)
  → shared_head(all active nodes) → cleanup_crdt_temp(before head)
  → compact_crdt_docs(merge per-zettel) → git gc
```

## 16. File Attachments — `zdb-core/src/attachments.rs`

The attachments module provides file attachment support for zettels. Files are stored in `reference/{zettel_id}/` within the git repo and tracked in the zettel's frontmatter `attachments` array.

### Storage Model

Attached files live alongside zettel Markdown files but in a separate `reference/` tree:

```
zettelkasten/20260301130000.md     # the zettel
reference/20260301130000/photo.jpg  # its attachment
reference/20260301130000/resume.pdf # another attachment
```

The zettel's frontmatter tracks metadata:

```yaml
attachments:
  - name: photo.jpg
    mime: image/jpeg
    size: 45230
  - name: resume.pdf
    mime: application/pdf
    size: 128400
```

```bash
grep -n "pub fn\|pub struct" zdb-core/src/attachments.rs
```

```rust
58:pub fn list_attachments(repo: &GitRepo, id: &ZettelId) -> Result<Vec<AttachmentInfo>> {
66:pub fn attach_file(
121:pub fn detach_file(
```

### Core API

Three public functions compose git operations and frontmatter manipulation:

- **`attach_file`** — writes binary file to `reference/{id}/{filename}` via `commit_binary_file`, updates the zettel's frontmatter `attachments` array, re-indexes
- **`detach_file`** — removes the file from git via `delete_file`, strips the entry from frontmatter, re-indexes
- **`list_attachments`** — parses the zettel's frontmatter and returns `AttachmentInfo` structs

All three depend on the parser's round-trip fidelity: read zettel → parse → modify `meta.extra["attachments"]` → serialize → commit.

### MIME Detection

`AttachmentInfo::mime_from_filename()` maps file extensions to MIME types via a simple match expression — no external dependency. Falls back to `application/octet-stream` for unknown extensions.

### Index Table

The indexer creates `_zdb_attachments(zettel_id, name, mime, size, path)` and populates it during `index_zettel` from the frontmatter `attachments` array. This enables SQL queries like:

```sql
SELECT z.title, a.name, a.size FROM zettels z
JOIN _zdb_attachments a ON a.zettel_id = z.id
WHERE a.mime LIKE 'image/%'
```

### API Surface

- **CLI**: `zdb attach <id> <file>`, `zdb detach <id> <filename>`, `zdb attachments <id>`
- **GraphQL**: `attachFile(input: AttachFileInput!)` mutation (base64 data), `detachFile(zettelId, filename)`, `attachments` field on `Zettel` type
- **REST**: `GET /attachments/{zettel_id}/{filename}` serves files directly with correct Content-Type
- **FFI**: `attach_file(zettel_id, file_path)`, `detach_file(zettel_id, filename)`, `list_attachments(zettel_id)` on `ZettelDriver`

## 17. Zettel Rename — `zdb-core/src/git_ops.rs` + `parser.rs` + `indexer.rs`

When a zettel moves (e.g. gains a type and relocates from `zettelkasten/` to `zettelkasten/contact/`), all links across the repo pointing to the old path or bare ID must be rewritten. The rename feature spans three modules:

- **parser** — `rewrite_links()` performs string-level link target replacement across wikilinks, markdown links, and embeds
- **indexer** — `backlinking_zettel_paths()` finds all zettels linking to a target
- **git_ops** — `rename_file()` does the git mv, `rename_zettel()` orchestrates the full operation

### Link Rewriting

The parser module provides `rewrite_links()` which replaces link targets in raw file content. It handles three link syntaxes:

1. **Wikilinks** — `[[old_target]]` and `[[old_target|display]]`, preserving display text and YAML quoting
2. **Markdown links** — `[display](old_target)` → `[display](new_target)`
3. **Embeds** — `![[old_target]]` and `![[old_target|display]]`

Bare URLs are not rewritten (they are external references).
```

### Backlink Path Resolution

The indexer adds `backlinking_zettel_paths()` which joins `_zdb_links` with `zettels` to return both source ID and file path for each backlinking zettel:

```bash
sed -n '/pub fn backlinking_zettel_paths/,/^    }/p' zdb-core/src/indexer.rs
```

```rust
    pub fn backlinking_zettel_paths(&self, target: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT l.source_id, z.path \
             FROM _zdb_links l JOIN zettels z ON l.source_id = z.id \
             WHERE l.target_path = ?1",
        )?;
        let rows = stmt.query_map(params![target], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
```

### Rename Orchestration

`rename_zettel()` in `git_ops.rs` ties everything together:

1. Move the file via `rename_file()` (first commit)
2. Extract bare ID from the old path
3. Query backlinks for both old path and bare ID (links may use either form)
4. Deduplicate, then rewrite each backlinking file using `rewrite_links()` (handles wikilinks, markdown links, and embeds)
5. Commit all rewritten files in a single batch (second commit)
6. Refresh the index via `rebuild_if_stale()`, then query `broken_backlinks()` filtered to the old path/ID to detect any references that weren't rewritten
7. Return a `RenameReport` with `updated` (rewritten files) and `unresolvable` (remaining broken references)

```bash
sed -n '/^pub fn rename_zettel/,/^}/p' zdb-core/src/git_ops.rs
```

```rust
pub fn rename_zettel(
    repo: &GitRepo,
    index: &crate::indexer::Index,
    old_path: &str,
    new_path: &str,
) -> Result<RenameReport> {
    // Step 1: move the file
    repo.rename_file(old_path, new_path, &format!("rename: {old_path} → {new_path}"))?;

    // Extract the bare ID from the old path (filename without .md)
    let old_id = Path::new(old_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Step 2: find backlinks for both old path and bare ID
    let mut backlinks = index.backlinking_zettel_paths(old_path)?;
    if !old_id.is_empty() && old_id != old_path {
        let by_id = index.backlinking_zettel_paths(old_id)?;
        for entry in by_id {
            if !backlinks.iter().any(|(id, _)| *id == entry.0) {
                backlinks.push(entry);
            }
        }
    }

    let mut report = RenameReport::default();

    if backlinks.is_empty() {
        return Ok(report);
    }

    // Derive new target forms for rewriting
    let new_target_for_path = new_path.trim_end_matches(".md");
    let old_target_for_path = old_path.trim_end_matches(".md");

    // Step 3: rewrite each backlinking file
    let mut writes: Vec<(String, String)> = Vec::new();
    for (_source_id, source_path) in &backlinks {
        let content = repo.read_file(source_path)?;
        let mut rewritten = content.clone();

        // Rewrite path-qualified links (without .md, as wikilinks typically omit it)
        rewritten = crate::parser::rewrite_links(&rewritten, old_target_for_path, new_target_for_path);

        // Rewrite bare ID links
        if !old_id.is_empty() {
            rewritten = crate::parser::rewrite_links(&rewritten, old_id, new_target_for_path);
        }

        if rewritten != content {
            writes.push((source_path.clone(), rewritten));
            report.updated.push(source_path.clone());
        }
    }

    // Step 4: commit all rewrites in one batch
    if !writes.is_empty() {
        let write_refs: Vec<(&str, &str)> = writes.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        repo.commit_files(&write_refs, &format!("refactor: rewrite links after rename {old_path}"))?;
    }

    // Step 5: detect remaining broken references to old target
    index.rebuild_if_stale(repo)?;
    let old_targets = [old_path, old_target_for_path, old_id];
    report.unresolvable = index
        .broken_backlinks()?
        .into_iter()
        .filter(|(_src, target)| old_targets.contains(&target.as_str()))
        .filter_map(|(src_id, _)| index.resolve_path(&src_id).ok())
        .collect();

    Ok(report)
}
```

### CLI Command

The `zdb rename <id> <new-path>` command resolves the current path from the index, calls `rename_zettel()`, and prints the report:

- **`zdb rename <id> <new-path>`** — Move a zettel and rewrite all backlinks pointing to it

## 18. Multi-Device Simulation Tests — `tests/e2e/multi_device.rs`


The multi-device e2e tests validate sync correctness under adversarial conditions: concurrent edits, delete-vs-edit conflicts, and chaotic random operations across multiple nodes. All tests use the `MultiNodeSetup` harness from `common.rs`, which creates isolated git repos linked through a bare "hub" remote.

### Test Harness Additions

The `MultiNodeSetup` gained a `delete` helper alongside its existing `create`, `update`, `read`, and `sync` methods:

```bash
sed -n '444,450p' tests/e2e/common.rs
```

```rust
    /// Delete a zettel
    pub fn delete(node: &Path, id: &str) {
        ZdbTestRepo::zdb_at(node)
            .args(["delete", id])
            .assert()
            .success();
    }
```

### Concurrent Edits — `concurrent_edits_same_zettel`

Three nodes all edit the same zettel independently, then sync. The test asserts CRDT determinism — all nodes converge to identical content regardless of sync order:

```bash
sed -n '317,343p' tests/e2e/multi_device.rs
```

```rust
fn concurrent_edits_same_zettel() {
    let setup = MultiNodeSetup::new(3);

    // Node0 creates a zettel, sync to all
    let id = MultiNodeSetup::create(&setup.nodes[0], "Shared zettel", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // All 3 nodes edit the same zettel without syncing between edits
    MultiNodeSetup::update(&setup.nodes[0], &id, "Edit from node0", "body from node0");
    MultiNodeSetup::update(&setup.nodes[1], &id, "Edit from node1", "body from node1");
    MultiNodeSetup::update(&setup.nodes[2], &id, "Edit from node2", "body from node2");

    // Sync cascade: 3 rounds to ensure full convergence
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // All nodes must converge to identical content (CRDT determinism)
    let content0 = MultiNodeSetup::read(&setup.nodes[0], &id);
    let content1 = MultiNodeSetup::read(&setup.nodes[1], &id);
    let content2 = MultiNodeSetup::read(&setup.nodes[2], &id);

    assert_eq!(content0, content1, "node0 and node1 diverged");
    assert_eq!(content1, content2, "node1 and node2 diverged");
}
```

### Delete-vs-Edit — `delete_vs_edit_multi_node`

Tests the "edit wins" conflict policy across 3 nodes. Node1 deletes a zettel while node2 edits it. After sync, the edit survives with a `resurrected: true` frontmatter marker:

```bash
sed -n '348,387p' tests/e2e/multi_device.rs
```

```rust
fn delete_vs_edit_multi_node() {
    let setup = MultiNodeSetup::new(3);

    // Node0 creates a zettel, sync to all
    let id = MultiNodeSetup::create(&setup.nodes[0], "Will conflict", "original body");
    MultiNodeSetup::push(&setup.nodes[0]);
    MultiNodeSetup::sync(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Node1 deletes the zettel
    MultiNodeSetup::delete(&setup.nodes[1], &id);

    // Node2 edits the zettel
    MultiNodeSetup::update(&setup.nodes[2], &id, "Edited after delete", "surviving body");

    // Node1 pushes delete, then node2 syncs (triggers delete-vs-edit conflict)
    MultiNodeSetup::push(&setup.nodes[1]);
    MultiNodeSetup::sync(&setup.nodes[2]);

    // Full sync to propagate resolution
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // Edit wins: zettel should exist on all nodes with node2's content
    for (i, node) in setup.nodes.iter().enumerate() {
        let out = MultiNodeSetup::read(node, &id);
        assert!(
            out.contains("Edited after delete") || out.contains("surviving body"),
            "node {i}: edit should win over delete, got: {out}"
        );
    }

    // Check resurrected marker in frontmatter
    let out = MultiNodeSetup::read(&setup.nodes[2], &id);
    assert!(
        out.contains("resurrected: true"),
        "resurrected marker missing from frontmatter: {out}"
    );
}
```

### Chaos Convergence — `chaos_convergence`

The stress test: 4 nodes each perform 5 random operations (create or update), then sync until convergence. Uses a seeded RNG (`StdRng::seed_from_u64(42)`) for deterministic replay. Two helper functions read zettel files directly from disk to compare state across nodes:

```bash
sed -n '392,413p' tests/e2e/multi_device.rs
```

```rust
fn list_zettels(node: &std::path::Path) -> Vec<String> {
    let zk_dir = node.join("zettelkasten");
    let mut files: Vec<String> = std::fs::read_dir(&zk_dir)
        .unwrap()
        .filter_map(|e| {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !name.starts_with('_') {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    files.sort();
    files
}

/// Read a zettel file directly from disk for comparison.
fn read_zettel_file(node: &std::path::Path, filename: &str) -> String {
    std::fs::read_to_string(node.join("zettelkasten").join(filename)).unwrap()
}
```

The test runs in four phases: seed each node with one zettel, sync all, random operations, then converge and verify. The convergence check asserts both the zettel file set and file contents are identical across all 4 nodes:

```bash
sed -n '416,509p' tests/e2e/multi_device.rs
```

```rust
fn chaos_convergence() {
    let setup = MultiNodeSetup::new(4);
    let mut rng = StdRng::seed_from_u64(42);

    // Each node tracks its locally-known zettel IDs (for updates)
    let mut local_ids: Vec<Vec<String>> = vec![vec![]; 4];

    // Phase 1: each node creates an initial zettel so there's something to operate on
    for (i, node) in setup.nodes.iter().enumerate() {
        let id = MultiNodeSetup::create(node, &format!("Init {i}"), &format!("body {i}"));
        local_ids[i].push(id);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Sync all so every node knows every zettel
    for _ in 0..3 {
        sync_round_robin(&setup);
    }

    // Propagate all IDs to all nodes' known lists
    let all_ids: Vec<String> = local_ids.iter().flatten().cloned().collect();
    for ids in &mut local_ids {
        *ids = all_ids.clone();
    }

    // Phase 2: each node performs 5 random ops (create or update only)
    for i in 0..4 {
        for _ in 0..5 {
            let op: u8 = rng.gen_range(0..3);
            match op {
                0 => {
                    // Create
                    let id = MultiNodeSetup::create(
                        &setup.nodes[i],
                        &format!("Chaos {i}"),
                        &format!("chaos body {i}"),
                    );
                    local_ids[i].push(id);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                1 if !local_ids[i].is_empty() => {
                    // Update a random known zettel
                    let idx = rng.gen_range(0..local_ids[i].len());
                    let id = local_ids[i][idx].clone();
                    MultiNodeSetup::update(
                        &setup.nodes[i],
                        &id,
                        &format!("Updated by {i}"),
                        &format!("updated body {i}"),
                    );
                }
                _ => {
                    // Create (fallback when no IDs to update)
                    let id = MultiNodeSetup::create(
                        &setup.nodes[i],
                        &format!("Chaos fallback {i}"),
                        &format!("fallback body {i}"),
                    );
                    local_ids[i].push(id);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    }

    // Phase 3: converge with multiple sync rounds
    for _ in 0..5 {
        sync_round_robin(&setup);
    }

    // Phase 4: verify all nodes have identical zettel set and content
    let files_node0 = list_zettels(&setup.nodes[0]);
    assert!(!files_node0.is_empty(), "node 0 should have zettels");

    for (i, node) in setup.nodes.iter().enumerate().skip(1) {
        let files = list_zettels(node);
        assert_eq!(
            files_node0, files,
            "node 0 and node {i} have different zettel sets"
        );
    }

    // Verify file contents match across all nodes
    for filename in &files_node0 {
        let content0 = read_zettel_file(&setup.nodes[0], filename);
        for (i, node) in setup.nodes.iter().enumerate().skip(1) {
            let content = read_zettel_file(node, filename);
            assert_eq!(
                content0, content,
                "node 0 and node {i} diverged on {filename}"
            );
        }
    }
}
```


## 19. Broken Backlink Report — `zdb-cli/src/main.rs`


When a zettel is deleted, other zettels may still link to it via wikilinks. The `zdb delete` command reports these broken backlinks to stderr after deletion.

### Delete Handler

Before removing a zettel from git and the index, the delete handler queries `backlinking_zettel_paths()` with the zettel ID. This must happen before `remove_zettel()` because the link data lives in the index. After deletion, any broken backlinks are printed as warnings:

```bash
sed -n '/Command::Delete { id }/,/^        }/p' zdb-cli/src/main.rs
```

```rust
        Command::Delete { id } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;
            let path = index.resolve_path(&id)?;
            let broken = index.backlinking_zettel_paths(&id)?;
            repo.delete_file(&path, &format!("delete zettel {id}"))?;
            index.remove_zettel(&id)?;
            redb_remove_zettel(&cli.repo, &id);
            if !broken.is_empty() {
                eprintln!(
                    "warning: {} zettel(s) have broken backlinks after deleting {id}:",
                    broken.len()
                );
                for (src_id, src_path) in &broken {
                    eprintln!("  - {src_id} ({src_path})");
                }
            }
        }
```


The key ordering is: query backlinks → delete from git → remove from index → print report. Querying before deletion ensures the link data is still available in the index.

### Status Integration

`zdb status` reports resurrected zettels (from delete-vs-edit conflicts) via `Index::resurrected_zettels()`, which queries `_zdb_fields` for `resurrected = 'true'`. It also reports broken backlinks using `broken_backlinks()`:

```bash
sed -n '/let broken = index.broken_backlinks/,/^                }/p' zdb-cli/src/main.rs
```

```rust
                let broken = index.broken_backlinks().unwrap_or_default();
                if !broken.is_empty() {
                    println!("broken backlinks:");
                    for (src_id, target_path) in &broken {
                        println!("  {src_id} -> {target_path}");
                    }
                }
```

## 20. CI Test Workflow and Cross-Platform Path Tests

CI is split into three tiers: PR, nightly/full-validation, and release.

**PR workflow** (`.github/workflows/test.yml`) — Linux-only, targets ~5 min wall clock. Runs `cargo clippy`, `cargo test-ci` (unit/bin tests), `SMOKE_PROFILE=quick`, `sync_test`, and a curated e2e subset covering GraphQL CRUD, PgWire, WebSocket subscriptions, sync/CRDT conflict resolution, and multi-device resync.

```yaml
      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings

      - name: Build CLI binary
        run: cargo build -p zdb-cli --bin zdb

      - name: Fast cargo test tier
        run: cargo test-ci

      - name: Quick smoke test
        env:
          SMOKE_PROFILE: quick
          ZDB_BIN: ${{ github.workspace }}/target/debug/zdb
        run: ./tests/smoke.sh

      - name: Sync integration tests
        run: cargo test -p zdb-core --test sync_test

      - name: E2E - GraphQL CRUD
        run: cargo test -p zdb-e2e --test e2e serve::crud_lifecycle -- --exact
```

**Full validation** (`.github/workflows/full-validation.yml`) — reusable workflow called by nightly and release. Runs `linux-clippy`, `linux-core` (sync_test + property_tests), `linux-thresholds` (query/growth/sync thresholds in `--release`), `linux-smoke` (full smoke), `linux-e2e` (full e2e suite), `macos-full`, and `windows-full`. On success, `mark-validated` posts a `full-validation/passed` commit status on the SHA.

**Nightly** (`.github/workflows/nightly.yml`) — scheduled driver with a guard job that checks for an existing `full-validation/passed` commit status on the current SHA. Skips heavy jobs when the SHA is already validated. Supports `force` input to re-run regardless.

**Release** (`.github/workflows/release.yml`) — tag-push trigger. Dereferences the tag to a commit SHA, checks for `full-validation/passed`. If found, skips tests and proceeds to build/package. If not, calls full-validation first.

```bash
sed -n '1,22p' tests/smoke.sh
```

```bash
SMOKE_PROFILE="${SMOKE_PROFILE:-full}"
case "$SMOKE_PROFILE" in
  quick|full) ;;
  *)
    echo "unknown SMOKE_PROFILE: $SMOKE_PROFILE (expected quick or full)" >&2
    exit 2
    ;;
esac

PREP_LABEL="prebuilt binary"
if [ -z "${ZDB_BIN:-}" ]; then
  cargo build --quiet
  if [ "$SMOKE_PROFILE" = "full" ]; then
    cargo clippy --workspace --quiet
    cargo bench --no-run --quiet 2>/dev/null
    PREP_LABEL="clippy + bench compile"
  else
    PREP_LABEL="build"
  fi
fi
ZDB="${ZDB_BIN:-$(cargo metadata --format-version=1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug/zdb}"
```

The important detail is the absolute `ZDB_BIN`. `tests/smoke.sh` changes into a temporary directory before invoking the CLI, so a relative path like `target/debug/zdb` would work only from the repository root and then break as soon as the smoke script moves into its sandbox. Passing the workspace-absolute path preserves the single-build optimization and keeps the smoke scripts portable across Linux, macOS, and Windows.

The workspace default excludes `zdb-e2e` and `zdb-uniffi-bindgen` from plain `cargo test`, so pre-commit runs no longer drag the external harness into every loop. PR CI runs only on Linux: `test-ci` (unit/bin coverage via `--lib --bins`), `SMOKE_PROFILE=quick`, `sync_test`, and five curated e2e tests. Property tests and threshold tests run only in full-validation (nightly/release). All threshold tests use `#[cfg_attr(debug_assertions, ignore)]` so they skip in debug builds and only execute in the `linux-thresholds` release-mode job. The temp-repo helpers disable `commit.gpgsign` in their local `.git/config` — `bundle.rs` uses the `git2` config API while the e2e harness shells out to `git config` — which prevents shell-based `git merge` / `git commit` steps from inheriting a developer's global GPG-signing policy and breaking otherwise-isolated tests. On Windows, the PowerShell smoke helper normalizes native command output into a single string before regex assertions, and the backlink delete/status checks must go through that helper rather than raw `2>&1`, which avoids the `-notmatch`/array behavior that can falsely fail multiline CLI output.

```bash
sed -n '892,909p' zdb-core/src/git_ops.rs
```

```rust
    use super::*;
    use tempfile::TempDir;

    fn temp_repo() -> (TempDir, GitRepo) {
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        (dir, repo)
    }

    fn native_absolute_path() -> &'static str {
        if cfg!(windows) {
            r"C:\Windows\System32\drivers\etc\hosts"
        } else {
            "/etc/passwd"
        }
    }

    #[test]
```

```bash
sed -n '1231,1245p' zdb-core/src/git_ops.rs
```

```rust
    #[test]
    fn absolute_path_write_rejected() {
        let (_dir, repo) = temp_repo();
        let err = repo
            .commit_file(native_absolute_path(), "hacked", "write outside repo")
            .unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
    }

    #[test]
    fn absolute_path_read_rejected() {
        let (_dir, repo) = temp_repo();
        let err = repo.read_file(native_absolute_path()).unwrap_err();
        assert!(matches!(err, ZettelError::InvalidPath(_)));
    }
```

The path-validation logic itself did not change. What changed is the test fixture: the absolute-path regression tests now call `native_absolute_path()` so Windows exercises a real Windows absolute path instead of Unix-only literals like `/etc/passwd`. That keeps the `validate_path()` contract portable without weakening the repository-boundary checks described earlier in the walkthrough.

## Benchmark Validation Suite

The project validates performance against spec targets (NFR-01 through NFR-04, AC-19) using a combination of Criterion benchmarks and threshold assertion tests.

### Benchmark Structure

Five Criterion benchmark targets measure different aspects:

```bash
sed -n '/\[\[bench\]\]/,/^$/{ /name/p }' zdb-core/Cargo.toml
```

```toml
name = "crud"
name = "search"
name = "sync"
name = "growth"
name = "large_scale"
```

- **crud** — CRUD operations at 1K zettels
- **search** — FTS5 search and SQL SELECT at 1K and 5K zettels
- **sync** — fast-forward sync and compaction at 1K and 5K
- **growth** — repo size growth simulation (365 days × 10 edits/day at 5K)
- **large_scale** — FTS5 and SQL at 50K zettels

### Threshold Tests

Alongside Criterion benchmarks, three test files enforce NFR targets with hard assertions. All threshold tests use `#[cfg_attr(debug_assertions, ignore)]` so they skip in debug builds and run only in the `linux-thresholds` job under `--release`:

```bash
grep -E '^(#\[test\]|#\[ignore|fn [a-z])' zdb-core/tests/query_thresholds.rs
```

```rust
fn zettel_content(i: usize) -> String {
fn zettel_path(i: usize) -> String {
fn setup(count: usize) -> (TempDir, Index) {
fn median_ms<F: FnMut()>(mut f: F) -> u128 {
#[test]
fn nfr01_fts_query_under_10ms_at_5k() {
#[test]
fn nfr01_sql_query_under_10ms_at_5k() {
#[test]
#[ignore = "50K setup takes minutes — run explicitly"]
fn ac19_fts_query_under_50ms_at_50k() {
#[test]
#[ignore = "50K setup takes minutes — run explicitly"]
fn ac19_sql_query_under_50ms_at_50k() {
```

```bash
grep -E '^(#\[test\]|#\[ignore|fn [a-z])' zdb-core/tests/sync_thresholds.rs
```

```rust
fn zettel_content(i: usize) -> String {
fn zettel_path(i: usize) -> String {
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "sync thresholds require --release; debug runs are too slow"
)]
fn nfr03_sync_under_2s_at_5k() {
```

NFR-03 passes at ~120ms (threshold: 2000ms) after sync optimizations (incremental reindex, single push, deferred commit-graph).

```bash
sed -n '/^fn median_ms/,/^}/p' zdb-core/tests/query_thresholds.rs
```

```rust
fn median_ms<F: FnMut()>(mut f: F) -> u128 {
    // warmup
    for _ in 0..WARMUP_ITERS {
        f();
    }
    // measure
    let mut times = Vec::with_capacity(MEASURE_ITERS);
    for _ in 0..MEASURE_ITERS {
        let start = Instant::now();
        f();
        times.push(start.elapsed().as_millis());
    }
    times.sort();
    times[MEASURE_ITERS / 2]
}
```

The server crate adds a Criterion benchmark suite (`zdb-server/benches/server_load.rs`) measuring read latency, concurrent throughput, mixed read/write load, and cross-protocol comparison. This is the first benchmark targeting the server layer rather than zdb-core directly.

### Benchmark Harness

The server benchmark spawns a real `zdb serve` subprocess (just like e2e tests), seeds it with 200 or 5,000 zettels, then measures via HTTP (reqwest) and pgwire (tokio-postgres). A lightweight `BenchServer` struct mirrors the e2e `ServerGuard` pattern:

```bash
sed -n "/^struct BenchServer/,/^}/p" zdb-server/benches/server_load.rs
```

```rust
struct BenchServer {
    child: Child,
    port: u16,
    token: String,
    _dir: TempDir,
    _remote_dir: TempDir,
}
```

### Benchmark Groups

The suite contains nine benchmark groups across two scales — five at 200 zettels and four at 5,000 — measuring different aspects of server performance:

```bash
grep -E "benchmark_group\(\"" zdb-server/benches/server_load.rs | sed "s/.*benchmark_group(\"/- /" | sed "s/\".*//"
```

```text
- server/single_read
- server/concurrent_reads
- server/concurrent_search
- server/mixed_load
- server/protocol_comparison
- server_5k/single_read
- server_5k/concurrent_reads
- server_5k/mixed_load
- server_5k/protocol_comparison
```

- **server/single_read** — baseline latency for individual GraphQL queries (list, get, search)
- **server/concurrent_reads** — list query latency at 1/4/8/16 concurrent readers, measures actor serialization cost
- **server/concurrent_search** — FTS search at same concurrency levels
- **server/mixed_load** — read latency with and without background write mutations (includes sync every 3rd iteration)
- **server/protocol_comparison** — same get-zettel query across GraphQL, REST, NoSQL, and pgwire
- **server_5k/single_read** — repeats single-read benchmarks at 5,000 zettels for NFR-01 validation at target scale
- **server_5k/concurrent_reads** — repeats concurrent read benchmarks at 5,000 zettels
- **server_5k/mixed_load** — repeats mixed-load benchmarks at 5,000 zettels to validate operating envelope claims
- **server_5k/protocol_comparison** — repeats protocol comparison at 5,000 zettels

### Mixed-Load Pattern

The mixed-load benchmark spawns a background write loop that continuously creates, updates, and syncs zettels while the benchmark measures read latency. This reveals actor contention:

```bash
grep -A5 "fn spawn_write_load" zdb-server/benches/server_load.rs
```

```rust
fn spawn_write_load(
    rt: &tokio::runtime::Runtime,
    client: &reqwest::Client,
    url: &str,
    stop: Arc<AtomicBool>,
) -> (tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
```

The write loop alternates between `createZettel` and `updateZettel` mutations with a 1ms yield between operations, and runs a sync mutation every third iteration to exercise fetch/merge under mixed load. Criterion measures read latency both with and without this background load, producing a direct comparison.

### Cross-Protocol Comparison

The protocol comparison benchmark runs the same get-zettel-by-id query through all four server protocols with identical configuration. For pgwire, a persistent `tokio_postgres::Client` connection is reused across iterations to avoid measuring connection setup:

```bash
grep 'bench_function' zdb-server/benches/server_load.rs
```

### Concurrency Serialization Test

The `sync_during_writes_serialized_through_actor` e2e test in `tests/e2e/server_mutations.rs` validates that concurrent mutations serialize correctly through the actor. It spawns five concurrent writers and one sync thread, then verifies all creates succeed and each produced a distinct commit:

```bash
sed -n "/Verify serialization: all 5/,/^}/p" tests/e2e/server_mutations.rs
```

```rust
    // Verify serialization: all 5 zettels were created and are queryable
    assert_eq!(
        created_ids.len(),
        5,
        "expected 5 created IDs, got {}: {:?}",
        created_ids.len(),
        created_ids
    );

    for id in &created_ids {
        let query = format!(r#"{{ zettel(id: "{id}") {{ id title }} }}"#);
        let result = server.graphql(&query);
        assert!(
            result.get("errors").is_none(),
            "zettel {id} not found after concurrent writes: {result}"
        );
        assert_eq!(
            result.pointer("/data/zettel/id").and_then(|v| v.as_str()),
            Some(id.as_str()),
            "zettel {id} returned wrong data: {result}"
        );
    }

    // Verify serialization: each create produced a distinct commit
    let post_count = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("git rev-list failed");
    let commits_after: usize = String::from_utf8_lossy(&post_count.stdout)
        .trim()
        .parse()
        .unwrap();
    let new_commits = commits_after - commits_before;
    assert!(
        new_commits >= 5,
        "expected at least 5 new commits (one per create), got {new_commits}"
    );
}
```

The test confirms single-writer semantics: five concurrent creates each produce a distinct commit (at least 5 new commits), and all five zettels are queryable after the concurrent operations complete.

## Search Fast Path and Release-Only Threshold Gates

The CI failure on GitHub Actions was not a functional search bug. The failing test was a hard 10 ms latency assertion running inside the default debug test suite on shared runners. Two changes address that without weakening the actual performance contract: plain `Index::search()` no longer pays for pagination bookkeeping it does not return, and the 5K threshold tests are now treated as release-profile validation rather than debug-suite gating.

```bash
sed -n '866,903p' zdb-core/src/indexer.rs
```

```rust
    /// Full-text search with snippets and ranking.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT z.id, z.title, z.path, snippet(_zdb_fts, 1, '<b>', '</b>', '...', 32), rank
             FROM _zdb_fts
             JOIN zettels z ON z.rowid = _zdb_fts.rowid
             WHERE _zdb_fts MATCH ?1
             ORDER BY rank",
        )?;

        let results = stmt.query_map(params![query], |row| {
            Ok(SearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                path: row.get(2)?,
                snippet: row.get(3)?,
                rank: row.get(4)?,
            })
        })?;

        let mut hits = Vec::new();
        for r in results {
            hits.push(r?);
        }

        Ok(hits)
    }

    /// Paginated full-text search with snippets, ranking, and total count.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        let mut stmt = self.conn.prepare(
```

```bash
sed -n '188,210p' zdb-core/src/ffi.rs
```

```rust
    }

    pub fn search(&self, query: String) -> Result<Vec<SearchResult>, ZdbError> {
        let index = self.index.lock().unwrap();
        let results = index.search(&query).map_err(ZdbError::from)?;
        Ok(results
            .into_iter()
            .map(|r| SearchResult {
                id: r.id,
                title: r.title,
                path: r.path,
                snippet: r.snippet,
                rank: r.rank,
            })
            .collect())
    }

    pub fn search_paginated(&self, query: String, limit: u32, offset: u32) -> Result<PaginatedSearchResult, ZdbError> {
        let index = self.index.lock().unwrap();
        let result = index.search_paginated(&query, limit as usize, offset as usize).map_err(ZdbError::from)?;
        Ok(PaginatedSearchResult {
            hits: result
                .hits
```

```bash
sed -n '69,100p' zdb-core/tests/query_thresholds.rs
```

```rust
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "performance thresholds require --release; debug CI runners are too noisy"
)]
fn nfr01_fts_query_under_10ms_at_5k() {
    let (_dir, index) = setup(ZETTEL_COUNT_5K);
    let ms = median_ms(|| {
        index.search("architecture").unwrap();
    });
    assert!(
        ms < NFR01_THRESHOLD_MS,
        "NFR-01: FTS query took {ms}ms, threshold is {NFR01_THRESHOLD_MS}ms"
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "performance thresholds require --release; debug CI runners are too noisy"
)]
fn nfr01_sql_query_under_10ms_at_5k() {
    let (_dir, index) = setup(ZETTEL_COUNT_5K);
    let ms = median_ms(|| {
        index
            .query_raw("SELECT id, title FROM zettels WHERE title LIKE '%architecture%' LIMIT 10")
            .unwrap();
    });
    assert!(
        ms < NFR01_THRESHOLD_MS,
```

```bash
sed -n '23,40p' docs/src/technical/performance.md
```

```md
## Query Latency (NFR-01 / AC-06 / AC-19)

| Scale | Operation | Target | Measured |
|-------|-----------|--------|----------|
| 5K zettels | FTS search | < 10ms | ~3.0ms |
| 5K zettels | SQL SELECT | < 10ms | ~6.1µs |
| 50K zettels | FTS search | < 50ms | not yet measured |
| 50K zettels | SQL SELECT | < 50ms | not yet measured |

Run 5K benchmarks: `cargo bench -p zdb-core --bench search -- "5k"`

Run 50K benchmarks: `cargo bench -p zdb-core --bench large_scale`

5K threshold tests: `cargo test --release -p zdb-core --test query_thresholds nfr01_`

50K threshold tests (slow): `cargo test --release -p zdb-core --test query_thresholds -- --ignored`

## Repo Growth (NFR-02 / AC-08)
```

## Release Script Performance Gate

If performance validation is meant to happen locally before shipping, it has to sit on the release path itself rather than in a separate checklist. The release script now runs the release-profile 5K query threshold tests as part of preflight, before any version bump, commit, tag, or push. That makes the local machine that cuts the release responsible for proving the NFR gate passed.

```bash
sed -n '1,120p' dev/bin/release
```

```bash
#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: dev/release [--dry-run] [--pre <suffix>] [patch|minor|major|local]"
  exit 1
}

run_preflight() {
  echo "running release preflight checks..."
  cargo check --workspace --quiet
  echo "running release performance thresholds..."
  cargo test --release -p zdb-core --test query_thresholds nfr01_
}

DRY_RUN=false
PRE=""
BUMP=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=true; shift ;;
    --pre)     PRE="$2"; shift 2 ;;
    patch|minor|major|local) BUMP="$1"; shift ;;
    *) usage ;;
  esac
done

[[ -z "$BUMP" ]] && usage

CARGO_FILES=(
  zdb-core/Cargo.toml
  zdb-cli/Cargo.toml
  zdb-server/Cargo.toml
  tests/Cargo.toml
)

# --- local install mode ---
if [[ "$BUMP" == "local" ]]; then
  cargo install --path zdb-cli
  echo "installed: $(zdb --version 2>/dev/null || echo 'zdb (version unknown)')"
  exit 0
fi

# --- read current version ---
CURRENT=$(sed -n 's/^version = "\(.*\)"/\1/p' zdb-core/Cargo.toml | head -1)
IFS='.' read -r MAJOR MINOR PATCH <<< "${CURRENT%%-*}"

# --- compute new version ---
case "$BUMP" in
  patch) PATCH=$((PATCH + 1)) ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
esac

NEW="${MAJOR}.${MINOR}.${PATCH}"
[[ -n "$PRE" ]] && NEW="${NEW}-${PRE}"
TAG="v${NEW}"

# --- dry run ---
if $DRY_RUN; then
  echo "${CURRENT} -> ${NEW} (tag: ${TAG})"
  exit 0
fi

# --- preflight checks ---
git pull --ff-only
if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree not clean" >&2
  exit 1
fi
if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "error: tag ${TAG} already exists" >&2
  exit 1
fi
run_preflight

# --- bump versions ---
for f in "${CARGO_FILES[@]}"; do
  sed -i.bak "s/^version = \".*\"/version = \"${NEW}\"/" "$f"
  rm -f "${f}.bak"
done

# --- update lockfile ---
cargo check --workspace --quiet

# --- commit, tag, push ---
git add "${CARGO_FILES[@]}" Cargo.lock
git commit -m "build(zdb): bump to ${TAG}"
git tag "$TAG"
git push origin HEAD --tags

echo "released ${TAG}"
```

## Growth Threshold Gate

The repo-growth limit is now enforced as a normal release-only integration test instead of a Criterion assertion embedded in a benchmark. The benchmark still measures growth, but the pass/fail policy moved into `growth_thresholds.rs`, and the release script now runs both the query-latency and repo-growth thresholds before it bumps versions, tags, or pushes.

## E2E Server Log Handling

The compact mutation failures that showed up during workspace validation were caused by the e2e harness, not by compaction itself. `ServerGuard` used stderr as a readiness channel and then dropped that pipe after startup; when the server later emitted a warning during `compact` on an unregistered repo, the actor-side logging path could fail under test. The harness now points tracing logs at a temp directory inside the test repo so runtime warnings no longer depend on that startup pipe remaining open.

```bash
sed -n '1,120p' zdb-core/tests/growth_thresholds.rs
```

```rust
use tempfile::TempDir;
use zdb_core::git_ops::GitRepo;

const INITIAL_ZETTELS: usize = 5000;
const DAYS: usize = 365;
const EDITS_PER_DAY: usize = 10;
const GROWTH_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024; // 50MB

fn zettel_content(i: usize) -> String {
    let word = match i % 5 {
        0 => "architecture",
        1 => "refactoring",
        2 => "deployment",
        3 => "performance",
        _ => "documentation",
    };
    format!(
        "---\ntitle: Note about {word} {i}\ndate: 2026-01-01\ntags:\n  - bench\n  - {word}\n---\n\
         This zettel discusses {word} in the context of item {i}.\n\
         ---\n- source:: bench-{i}"
    )
}

fn zettel_path(i: usize) -> String {
    format!("zettelkasten/{:014}.md", 20260101000000u64 + i as u64)
}

fn dir_size(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}

/// NFR-02 / AC-08: repo growth < 50MB/year at 5K zettels.
/// Run with: cargo test --release --test growth_thresholds
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "growth thresholds require --release; debug runs are too slow"
)]
fn nfr02_repo_growth_under_50mb_per_year_at_5k() {
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();

    let files: Vec<(String, String)> = (0..INITIAL_ZETTELS)
        .map(|i| (zettel_path(i), zettel_content(i)))
        .collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    repo.commit_files(&refs, "seed").unwrap();

    let size_before = dir_size(dir.path());

    for day in 0..DAYS {
        let batch: Vec<(String, String)> = (0..EDITS_PER_DAY)
            .map(|edit| {
                let idx = (day * EDITS_PER_DAY + edit) % INITIAL_ZETTELS;
                let content = format!(
                    "---\ntitle: Updated note {idx} day {day}\ndate: 2026-01-01\ntags:\n  - bench\n---\n\
                     Modified on day {day}, edit {edit}.\n\
                     ---\n- source:: bench-{idx}"
                );
                (zettel_path(idx), content)
            })
            .collect();
        let refs: Vec<(&str, &str)> = batch.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        repo.commit_files(&refs, &format!("day {day}")).unwrap();
    }

    let size_after = dir_size(dir.path());
    let growth = size_after - size_before;

    assert!(
        growth < GROWTH_THRESHOLD_BYTES,
        "NFR-02: repo grew {:.1}MB, threshold is {:.1}MB",
        growth as f64 / (1024.0 * 1024.0),
        GROWTH_THRESHOLD_BYTES as f64 / (1024.0 * 1024.0),
    );
}
```

```bash
sed -n '1,105p' zdb-core/benches/growth.rs
```

```rust
use std::path::Path;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;
use zdb_core::git_ops::GitRepo;

/// Simulate repo growth: create zettels, then modify them over time.
/// NFR-02 / AC-08: repo growth < 50MB/year at 5K zettels.
///
/// Strategy: start with 5K zettels, then simulate 365 days of edits
/// (10 modifications/day = 3650 commits) and measure repo size.
/// Using 10/day instead of 100/day to keep bench runtime reasonable;
/// the threshold is scaled proportionally.
const INITIAL_ZETTELS: usize = 5000;
const DAYS: usize = 365;
const EDITS_PER_DAY: usize = 10;

fn zettel_content(i: usize) -> String {
    let word = match i % 5 {
        0 => "architecture",
        1 => "refactoring",
        2 => "deployment",
        3 => "performance",
        _ => "documentation",
    };
    format!(
        "---\ntitle: Note about {word} {i}\ndate: 2026-01-01\ntags:\n  - bench\n  - {word}\n---\n\
         This zettel discusses {word} in the context of item {i}.\n\
         ---\n- source:: bench-{i}"
    )
}

fn zettel_path(i: usize) -> String {
    format!("zettelkasten/{:014}.md", 20260101000000u64 + i as u64)
}

fn dir_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}

fn bench_growth(c: &mut Criterion) {
    let mut group = c.benchmark_group("growth");
    // Only run once — this is a measurement, not a hot-path benchmark
    group.sample_size(10);

    group.bench_function("repo_size_after_1yr", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let repo = GitRepo::init(dir.path()).unwrap();

                // Seed with initial zettels
                let files: Vec<(String, String)> = (0..INITIAL_ZETTELS)
                    .map(|i| (zettel_path(i), zettel_content(i)))
                    .collect();
                let refs: Vec<(&str, &str)> =
                    files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
                repo.commit_files(&refs, "seed").unwrap();

                (dir, repo)
            },
            |(dir, repo)| {
                let size_before = dir_size(dir.path());

                // Simulate edits over a year
                for day in 0..DAYS {
                    let batch: Vec<(String, String)> = (0..EDITS_PER_DAY)
                        .map(|edit| {
                            let idx = (day * EDITS_PER_DAY + edit) % INITIAL_ZETTELS;
                            let content = format!(
                                "---\ntitle: Updated note {idx} day {day}\ndate: 2026-01-01\ntags:\n  - bench\n---\n\
                                 Modified on day {day}, edit {edit}.\n\
                                 ---\n- source:: bench-{idx}"
                            );
                            (zettel_path(idx), content)
                        })
                        .collect();
                    let refs: Vec<(&str, &str)> =
                        batch.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
                    repo.commit_files(&refs, &format!("day {day}")).unwrap();
                }

                let size_after = dir_size(dir.path());
                let growth = size_after - size_before;

                black_box(growth);
            },
        );
    });

    group.finish();
}

criterion_group!(benches, bench_growth);
criterion_main!(benches);
```

```bash
sed -n '40,70p' tests/e2e/common.rs
```

```rust
impl ServerGuard {
    pub fn start(repo: &ZdbTestRepo) -> Self {
        let port = SERVER_PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pg_port = SERVER_PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let log_dir = repo.path().join(".local/test-logs");

        let mut child = std::process::Command::new(zdb_bin())
            .arg("--repo")
            .arg(repo.path())
            .arg("--log-dir")
            .arg(&log_dir)
            .arg("serve")
            .arg("--port")
            .arg(port.to_string())
            .arg("--pg-port")
            .arg(pg_port.to_string())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start server");

        let stderr = child.stderr.take().unwrap();
        let reader = BufReader::new(stderr);
        let mut token = String::new();
        let mut http_ready = false;
        let mut pg_ready = false;

        for line in reader.lines() {
            let line = line.unwrap();
            if line.contains("auth token:") {
                if let Some(path) = line.split("auth token: ").nth(1) {
                    token = std::fs::read_to_string(path.trim())
```

## FFI: ZettelDriver Lifecycle (create_repo, register_node)

The FFI layer exposes `ZettelDriver` — a high-level facade wrapping `GitRepo` + `Index` behind mutexes. Two constructors and a node registration method enable mobile apps to bootstrap repos without the CLI:

- `create_repo(repo_path)` — initializes a new ZettelDB repo (directories, .gitignore, initial commit) then opens it
- `new(repo_path)` — opens an existing repo
- `register_node(name)` — registers a sync node, returning its UUID

These methods were added to eliminate the `Process()`/`ProcessBuilder` dependency in Swift/Kotlin tests, making them compatible with iOS simulator and Android emulator targets where shell access is unavailable.

```bash
sed -n '135,170p' zdb-core/src/ffi.rs
```

```rust
#[uniffi::export]
impl ZettelDriver {
    /// Open an existing ZettelDB repository.
    #[uniffi::constructor]
    pub fn new(repo_path: String) -> Result<Self, ZdbError> {
        let path = Path::new(&repo_path);
        let repo = GitRepo::open(path).map_err(ZdbError::from)?;
        let db_dir = path.join(".zdb");
        std::fs::create_dir_all(&db_dir)
            .map_err(|e| ZdbError::from(ZettelError::Io(e)))?;
        let db_path = db_dir.join("index.db");
        let index = Index::open(&db_path).map_err(ZdbError::from)?;
        Ok(Self {
            repo: Mutex::new(repo),
            index: Mutex::new(index),
            repo_path: path.to_path_buf(),
        })
    }

    /// Initialize a new ZettelDB repository at `repo_path` and open it.
    #[uniffi::constructor]
    pub fn create_repo(repo_path: String) -> Result<Self, ZdbError> {
        let path = Path::new(&repo_path);
        GitRepo::init(path).map_err(ZdbError::from)?;
        Self::new(repo_path)
    }

    pub fn create_zettel(&self, content: String, message: String) -> Result<String, ZdbError> {
        let parsed = parser::parse(&content, "new.md").map_err(ZdbError::from)?;
        let id = parsed
            .meta
            .id
            .as_ref()
            .map(|z| z.0.clone())
            .unwrap_or_else(|| parser::generate_id().0);
        let rel_path = format!("zettelkasten/{id}.md");
```

```bash
sed -n '254,263p' zdb-core/src/ffi.rs
```

```rust
        let repo = self.repo.lock().unwrap();
        let node = crate::sync_manager::register_node(&repo, &name).map_err(ZdbError::from)?;
        Ok(node.uuid)
    }

    pub fn compact(&self) -> Result<(), ZdbError> {
        let repo = self.repo.lock().unwrap();
        let sync_mgr = SyncManager::open(&repo).map_err(ZdbError::from)?;
        let opts = crate::types::CompactOptions {
            force: true,
            skip_backup: true, // mobile has no recovery workflow
            ..Default::default()
        };
        crate::compaction::compact(&repo, &sync_mgr, &opts).map_err(ZdbError::from)?;
        Ok(())
```

`create_repo` delegates to `GitRepo::init()` which creates the standard directory structure (`zettelkasten/`, `reference/`, `.nodes/`, `.crdt/temp/`), `.gitignore`, version file, and initial commit. Then it calls `new()` to open the repo and SQLite index.

`register_node` delegates to `sync_manager::register_node()` which generates a UUID, writes a `.nodes/{uuid}.toml` file, and stores the UUID locally in `.git/zdb-node`.

### Bundle Export via FFI

`ZettelDriver` exposes three bundle methods:

- `export_full_bundle(output_path)` — delegates to `bundle::export_full_bundle`, creates a tar with `target_node = "*"` containing all refs
- `export_delta_bundle(target_node_uuid, output_path)` — delegates to `bundle::export_bundle`, creates a tar containing only commits the target hasn't seen (based on `known_heads` in `.nodes/{uuid}.toml`)
- `import_bundle(bundle_path)` — delegates to `bundle::import_bundle`, unbundles, merges, resolves conflicts, and reindexes

All three follow the same lock pattern: acquire `repo` mutex, open `SyncManager`, delegate, map errors to `ZdbError`.

### Swift/Kotlin Test Compatibility

Both test suites now use `ZettelDriver.createRepo()` and `registerNode()` instead of shelling out to the `zdb` CLI binary. This removes the `Process()` dependency and makes tests portable to iOS simulator and Android instrumented test targets.

The delta bundle binding tests (`testDeltaBundleExportImport`) use shell commands (`git add`/`git commit`) to set up a fake remote node with `known_heads` — the FFI doesn't expose low-level node config writes, so tests write `.nodes/{uuid}.toml` directly and commit via git CLI.

## Embedded–Server SQL Parity

Both the server actor and `ZettelDriver` (FFI) delegate typed SQL execution to the same `SqlEngine` in `zdb-core`. The server constructs `SqlEngine::new(index, repo)` per command; the FFI path does the same per `execute_sql` call. This ensures identical Git-backed semantics for single-statement DDL and DML across both paths.

**Transaction difference**: The embedded path supports multi-statement transactions via `begin_transaction`/`commit_transaction`/`rollback_transaction`, which suspend and resume a `TransactionBuffer` across calls. The server creates a fresh `SqlEngine` per `executeSql` command, so BEGIN/COMMIT/ROLLBACK cannot span multiple GraphQL or pgwire calls — each statement executes atomically in isolation. Multi-statement transactions are an embedded-only capability.

```bash
sed -n "655,658p" zdb-server/src/actor.rs
```

```rust
        ActorCommand::ExecuteSql { sql } => {
            let mut engine = SqlEngine::new(index, repo);
            ActorReply::SqlResult(engine.execute(&sql))
        }
```

```bash
sed -n "353,374p" zdb-core/src/ffi.rs
```

```rust
    pub fn execute_sql(&self, sql: String) -> Result<SqlResultRecord, ZdbError> {
        let index = self.index.lock().unwrap();
        let repo = self.repo.lock().unwrap();
        let mut engine = crate::sql_engine::SqlEngine::new(&index, &*repo);

        // If a transaction is active, inject the buffered state.
        let mut txn_guard = self.txn.lock().unwrap();
        if let Some(buf) = txn_guard.take() {
            engine.resume_transaction(buf);
        }

        let result = engine.execute(&sql).map_err(|e| {
            // On error, preserve transaction state if still active.
            *txn_guard = engine.suspend_transaction();
            ZdbError::from(e)
        })?;

        // Preserve transaction state for subsequent calls.
        *txn_guard = engine.suspend_transaction();

        Ok(result.into())
    }
```

The FFI layer adds transaction buffer management around the same core `SqlEngine::execute` call. `SqlResultRecord` flattens the `SqlResult` enum into a UniFFI-safe struct. Transaction state (`TransactionBuffer`) is suspended from the ephemeral `SqlEngine` between calls and reinjected on the next call; the SAVEPOINT persists on `Index.conn` independently.

Type discovery (`list_type_schemas`) reads typedef zettels from Git and parses their schemas — the same data source the server uses to build its dynamic GraphQL schema.

## Cross-Process SQLite Safety — `busy_timeout`

The `Index::open` constructor configures WAL mode and a 5-second busy timeout on every SQLite connection. This prevents `SQLITE_BUSY` errors when the host app and widgets/extensions share the same index file across processes.

```bash
sed -n "23,26p" zdb-core/src/indexer.rs
```

```rust
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA busy_timeout=5000;")?;
```

WAL gives snapshot isolation: a reader always sees a consistent state even while another process writes. The busy timeout means if SQLite encounters a lock (e.g., during WAL checkpoint), it retries for up to 5 seconds instead of failing immediately.

## Idempotent Schema Bootstrap — `CREATE TABLE IF NOT EXISTS`

The SQL engine supports `IF NOT EXISTS` on `CREATE TABLE`, enabling idempotent schema bootstrap. This is critical for mobile host-shell apps where multiple feature modules register their tables at startup — each call is a safe no-op if the table already exists.

```bash
sed -n "331,359p" zdb-core/src/sql_engine.rs
```

```rust
    fn handle_create_table(&mut self, ct: &sqlparser::ast::CreateTable) -> Result<SqlResult> {
        let table_name = unquote_identifier(&ct.name.to_string());

        if is_reserved_table(&table_name) {
            return Err(ZettelError::SqlEngine(format!(
                "reserved table name: {table_name}"
            )));
        }

        // Check if typedef already exists
        let existing: Option<String> = self
            .index
            .conn
            .query_row(
                "SELECT id FROM zettels WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| row.get(0),
            )
            .ok();
        if existing.is_some() {
            if ct.if_not_exists {
                return Ok(SqlResult::Ok(format!(
                    "table already exists, skipped: {table_name}"
                )));
            }
            return Err(ZettelError::SqlEngine(format!(
                "table already exists: {table_name}"
            )));
        }
```

When `if_not_exists` is true (parsed from the SQL AST by sqlparser), the engine returns a success message instead of erroring. Without the flag, a duplicate table still produces an error. The check queries the existing typedef zettels in the index.

## Deployment Modes

ZettelDB supports three deployment modes, each suited to different environments:

1. **Server mode** (`zdb serve`): GraphQL/REST/PgWire over HTTP. Suited for web and desktop apps that talk to a local or networked backend. The server runs as a standalone process.

2. **Embedded native** (UniFFI `ZettelDriver`): The app links ZettelDB directly via FFI bindings (Swift/Kotlin). No server process needed. The app owns the git repo and SQLite index. Suited for single native apps.

3. **Host-shell model**: One installed mobile app with embedded `ZettelDriver`, shared across multiple feature modules. Each module contributes schema (`CREATE TABLE IF NOT EXISTS`), queries, and UI screens. The OS sees one app; users get several mini-apps. Required because mobile platforms sandbox apps and don't support shared local backends across installed apps.

## Stability Tiers

Every public-facing feature is classified as **stable** or **experimental**:

**Stable** (semver-protected from v0.1.0): CLI CRUD, search, query, sync, type management; Git storage format; FTS5; SQL DDL/DML; `zdb-core` public Rust API.

**Experimental** (may break in any release): GraphQL server, REST, PgWire, WebSocket, NoSQL API, UniFFI bindings, bundles, attachments, auto-update.

### How tiers are communicated

| Surface | Mechanism |
|---------|-----------|
| CLI `--help` | `[experimental]` prefix on subcommand descriptions |
| Server HTTP | `X-Experimental: true` response header (axum `map_response` layer) |
| Docs | Blockquote warnings on experimental feature pages |
| README | Dedicated stability section listing both tiers |

## Local Backlog PRDs

Not every unresolved spec-to-implementation gap belongs in the published docs immediately. The repository also keeps local planning artifacts under `.local/prds/backlog/` for work that has been identified, scoped, but not yet implemented.

At the moment that backlog includes PRDs for the major unresolved deltas discussed elsewhere in the docs: sparse index, fsmonitor-equivalent change tracking, background Git maintenance, pre-compaction backup export, NFR-03 sync performance, a server read fast path, and browser-compatible WebSocket auth. Those files are intentionally lightweight: each one captures the problem, implementation scope, success criteria, and non-goals so follow-up work can start without re-deriving the original context from the spec and deviation logs.
