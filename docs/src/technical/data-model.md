# Data Model

All types are defined in the `ddb-core/src/types/` directory (`mod.rs`, `value.rs`, `doogat.rs`, `schema.rs`).

## Repository Config

Repository-level settings stored in `.ddb.toml`:

```rust
pub struct RepoConfig {
    pub compaction: CompactionConfig,  // stale_ttl_days: 90, threshold_mb: 1
    pub crdt: CrdtConfig,             // default_strategy: "preset:default"
    pub maintenance: MaintenanceConfig,
}
```

### MaintenanceConfig

```rust
pub struct MaintenanceConfig {
    pub auto_enabled: bool,       // false
    pub write_threshold: u32,     // 50
}
```

Written on `init()` with defaults. Loaded via `GitRepo::load_config()` with serde defaults for missing fields.

## Identity

### DoogatId

```rust
pub struct DoogatId(pub String);
```

A 14-digit timestamp string (`YYYYMMDDHHmmss`), e.g. `"20260226120000"`. Custom `Deserialize` implementation accepts both YAML integer and string representations for backward compatibility.

## Doogat Structures

### DoogatMeta

Core metadata from YAML frontmatter:

```rust
pub struct DoogatMeta {
    pub id: Option<DoogatId>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub doogat_type: Option<String>,  // serialized as "type"
    pub tags: Vec<String>,
    pub extra: BTreeMap<String, serde_yaml::Value>,  // arbitrary additional fields
}
```

All fields are optional. The `extra` map captures any YAML fields not in the core schema, preserved through parse/serialize round-trips.

#### Reserved extra fields

The `attachments` key in `extra` is managed by the attachments module. It holds a list of `AttachmentInfo` records serialized as YAML maps:

```yaml
attachments:
  - name: diagram.png
    mime: image/png
    size: 48210
  - name: spec.pdf
    mime: application/pdf
    size: 102400
```

### AttachmentInfo

```rust
pub struct AttachmentInfo {
    pub name: String,
    pub mime: String,
    pub size: u64,
}
```

`mime_from_filename()` detects MIME type from extension (jpg, png, pdf, csv, md, html, etc.), falling back to `application/octet-stream`.

### File Storage

Attachment blobs live in the Git repository under `reference/{doogat_id}/`:

```
reference/
  20260226120000/
    diagram.png
    spec.pdf
```

These are committed as binary files alongside the doogat's frontmatter update. The `reference/` directory is a peer of `ddb/` in the repo root.

### Zone

Identifies which part of the doogat a piece of data comes from:

```rust
pub enum Zone {
    Frontmatter,
    Body,
    Reference,
}
```

### InlineField

A Dataview-style `key:: value` field extracted from body or reference zones:

```rust
pub struct InlineField {
    pub key: String,
    pub value: String,
    pub zone: Zone,
}
```

### Multi-Value Reference Fields

A doogat's reference section can contain multiple lines with the same key, representing a many-to-many relationship:

```markdown
## References
- category:: [[20260310120000]]
- category:: [[20260310120001]]
- category:: [[20260310120002]]
```

All lines are preserved through parse/serialize round-trips. During materialization, each line becomes a row in the corresponding junction table (e.g. `bookmark_category`). The SQL engine's INSERT and DELETE write-through operations on junction tables add and remove individual reference lines.

### LinkKind

Discriminant for the four supported link syntaxes:

```rust
pub enum LinkKind {
    WikiLink,      // [[target|display]]
    MarkdownLink,  // [title](url)
    Embed,         // ![[file#section|display]]
    BareUrl,       // https://example.com
}
```

### Link

A reference extracted from doogat content:

```rust
pub struct Link {
    pub target: String,
    pub display: Option<String>,
    pub section: Option<String>,
    pub kind: LinkKind,
    pub zone: Zone,
}
```

### ParsedDoogat

Full parsed representation of a doogat:

```rust
pub struct ParsedDoogat {
    pub meta: DoogatMeta,
    pub body: String,
    pub sections: Vec<Section>,       // parsed body sections
    pub reference_section: String,
    pub inline_fields: Vec<InlineField>,
    pub links: Vec<Link>,
    pub body_tags: Vec<String>,
    pub checkboxes: Vec<CheckboxItem>,
    pub path: String,
    pub updated_at: Option<String>,   // last-indexed timestamp (RFC 3339)
}
```

### Doogat

Raw three-zone split before metadata extraction:

```rust
pub struct Doogat {
    pub raw_frontmatter: String,
    pub body: String,
    pub reference_section: String,
}
```

## Sync Types

### NodeConfig

Per-device registration stored in `.nodes/{uuid}.toml`:

```rust
pub struct NodeConfig {
    pub uuid: String,
    pub name: String,
    pub known_heads: Vec<String>,  // Git commit OIDs this node has synced
    pub last_sync: Option<String>, // RFC3339 timestamp
    pub hlc: Option<String>,       // last HLC timestamp (clock continuity)
    pub status: NodeStatus,        // lifecycle status (default: Active)
    pub created: Option<String>,   // ISO 8601 registration timestamp
}
```

### NodeStatus

```rust
pub enum NodeStatus {
    Active,   // default
    Stale,
    Retired,
}
```

### MergeResult

Outcome of `git_ops::merge_remote()`:

```rust
pub enum MergeResult {
    AlreadyUpToDate,
    FastForward(Oid),
    Clean(Oid),
    Conflicts(Vec<ConflictFile>, Oid),  // conflicts + theirs OID
}
```

### ConflictFile

A file with merge conflicts, containing all three versions:

```rust
pub struct ConflictFile {
    pub path: String,
    pub ancestor: Option<String>,  // None if file is new
    pub ours: String,
    pub theirs: String,
    pub ours_hlc: Option<Hlc>,         // HLC from "ours" commit
    pub theirs_hlc: Option<Hlc>,       // HLC from "theirs" commit
    pub ours_blob_oid: Option<String>, // blob OID for binary conflict
    pub theirs_blob_oid: Option<String>,
}
```

### ResolvedFile

A conflict file after CRDT resolution:

```rust
pub struct ResolvedFile {
    pub path: String,
    pub content: String,
    pub fm_crdt_bytes: Option<Vec<u8>>, // serialized frontmatter CRDT state
}
```

## Type Definition Structures

### ColumnDef

A column in a typed table definition:

```rust
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,          // INTEGER, REAL, BOOLEAN, TEXT
    pub references: Option<String>, // FK target type name
    pub zone: Option<Zone>,         // which doogat zone this maps to
    pub required: bool,             // enforced during consistency checks
    pub search_boost: Option<f64>,  // FTS boost weight for bm25() ranking
    pub allowed_values: Option<Vec<String>>, // enum constraint
    pub default_value: Option<String>,       // default on INSERT
}
```

#### Enum columns

Columns with `allowed_values` emit a `CHECK(col IN (...))` constraint in materialized SQLite tables. The SQL engine validates values on INSERT and UPDATE, returning a `Validation` error on violation.

If `default_value` is set, the SQL engine fills it for omitted columns during INSERT.

YAML typedef example:

```yaml
columns:
  - name: status
    data_type: TEXT
    zone: frontmatter
    allowed_values:
      - todo
      - doing
      - done
    default_value: todo
```

### TableSchema

Schema for a materialized SQLite table:

```rust
pub struct TableSchema {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
    pub crdt_strategy: Option<String>,   // e.g. "preset:append-log"
    pub template_sections: Vec<String>,  // expected body section headings
    pub folder: bool,                    // store instances in ddb/{type}/ subdirectory
    pub stale_after_days: Option<u32>,   // stale discovery threshold
    pub title_template: Option<String>,  // title pattern for new instances
    pub origin: Option<String>,          // tracking label (e.g. PRD that created this type)
    pub unique_together: Option<Vec<Vec<String>>>, // composite unique constraints
}
```

### ConsistencyWarning

Advisory warnings collected during rebuild:

```rust
pub enum ConsistencyWarning {
    MalformedYaml { path: String, error: String },
    CrossZoneDuplicate { path: String, key: String },
    MissingRequired { path: String, type_name: String, field: String },
}
```

## Report Types

### RebuildReport

```rust
pub struct RebuildReport {
    pub indexed: usize,
    pub tables_materialized: usize,
    pub types_inferred: Vec<String>,
    pub warnings: Vec<ConsistencyWarning>,
}
```

### SyncReport

```rust
pub struct SyncReport {
    pub direction: String,          // "bidirectional", "up-to-date"
    pub commits_transferred: usize,
    pub conflicts_resolved: usize,
    pub resurrected: usize,
    pub collisions_reassigned: usize,
}
```

### CompactionReport

```rust
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
    pub backup_path: Option<PathBuf>,
}
```

## SQLite Schema

The search index (`indexer.rs`) uses these core tables:

```sql
-- Core doogat data
doogats(id TEXT PK, title, date, type, path UNIQUE, body, updated_at)

-- Tags (one row per tag per doogat)
_ddb_tags(doogat_id FK, tag)

-- Inline fields with zone tracking
_ddb_fields(doogat_id FK, key, value, zone)

-- Links with zone and kind tracking
_ddb_links(source_id FK, target_path, display, zone, kind TEXT DEFAULT 'wikilink')

-- Attachments (one row per file per doogat)
_ddb_attachments(doogat_id FK, name, mime, size INTEGER, path)

-- FTS5 full-text search (porter stemming, unicode61 tokenizer)
_ddb_fts(title, body, tags, fields)

-- Per-type FTS boost weight (max search_boost from typedef columns)
_ddb_boost(type_name TEXT PK, max_boost REAL DEFAULT 1.0)

-- Staleness tracking
_ddb_meta(key PK, value)  -- key="head", value=current Git HEAD OID
```

### Materialized Type Tables

During rebuild, the indexer creates additional tables for each typed doogat collection. For a type named `project`, the materialized table is:

```sql
project(id TEXT PK, completed INTEGER, deliverable TEXT, parent TEXT, ...)
```

Column types are derived from `_typedef` doogats (explicit) or inferred from data. These tables are ephemeral — dropped and recreated on each rebuild.
