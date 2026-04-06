# Doogat DB Code Walkthrough

Doogat DB is a hybrid Git-CRDT decentralized Doogat database written in Rust. Git is the source of truth for all data; a SQLite index with FTS5 provides fast reads and full-text search. When concurrent edits on different devices produce Git merge conflicts, an Automerge CRDT layer resolves them at the zone level. The system ships as a CLI, a multi-protocol server (GraphQL, REST, PgWire, WebSocket), and a UniFFI facade for native Swift/Kotlin apps.


## 1. Workspace Layout

The Cargo workspace lives at the repository root. Five members are declared in the root `Cargo.toml`; three of them are default members so that a bare `cargo build` compiles the fast local loop.

### ddb-core

All domain logic lives here: parsing, Git storage, CRDT conflict resolution, SQLite indexing, SQL translation, sync orchestration, compaction, attachments, and the UniFFI FFI facade. Every other crate depends on it. The crate root at `lib.rs` re-exports every public module and calls `uniffi::setup_scaffolding!()` to wire up FFI scaffolding. An optional `nosql` feature gate enables an experimental redb-backed key-value index. The `service` module provides a unified `DoogatService` orchestration layer that composes GitRepo, Index, and optional NoSQL into a single entry point — CLI, FFI, and server all delegate to it instead of independently composing core modules.

### ddb-cli

A thin clap-derived binary (`ddb`) in a single `main.rs`. It opens a `DoogatService` and delegates to it for all operations. Subcommands cover the full lifecycle: `init`, `create`, `read`, `update`, `delete`, `search`, `query`, `sync`, `reindex`, `compact`, `rename`, `serve`, `type`, `node`, `bundle`, `attach`, `detach`, `attachments`, `get`, `scan`, `backlinks`, `maintenance`, `discover`, `sequence`, and `update-bin`. An embedded `updater` module handles self-update from GitHub releases.

### ddb-server

An axum-based multi-protocol server library. Protocols: GraphQL (dynamic schema from typedef doogats), REST (JSON CRUD), PgWire (Postgres wire protocol for SQL clients), and WebSocket (GraphQL subscriptions). The crate wires up bearer-token auth, a single-writer actor, a read-only connection pool, an event bus for real-time subscriptions, and hot schema reload when typedef doogats change.

### ddb-uniffi-bindgen

An isolated binary crate whose sole purpose is to host the UniFFI bindgen tool. Keeping it separate avoids polluting ddb-core with binary targets and simplifies cross-compilation for Swift and Kotlin binding generation.

### tests (ddb-e2e)

An end-to-end test harness using `assert_cmd` for CLI tests and `reqwest` for server tests. E2E tests require the `ddb` binary on the PATH, so `cargo build -p ddb-cli` must run first. The crate lives in `tests/` and is declared as the fifth workspace member.

### Build aliases and test tiers

The workspace defines custom Cargo aliases:

- `cargo test` runs the fast local tier (unit tests in default members).
- `cargo test-ci` runs the bounded CI matrix (unit and binary targets only).
- `cargo test-full` runs the complete suite including workspace-wide and e2e tests.

Additional test surfaces include `tests/smoke.sh` and `tests/smoke.ps1` for CLI smoke tests, `tests/integration.sh` and `tests/integration.ps1` for full integration tests (server, sync, CRDT), and Criterion benchmarks in `ddb-core/benches/` for CRUD and search performance.


## 2. Core Data Model

All domain types are defined in `types.rs`. The data model is intentionally flat: every doogat is a Markdown file with structured frontmatter, and the system derives all relational structure from that content.

### DoogatId

A 14-digit timestamp string in the format `YYYYMMDDHHmmss`, for example `"20260226120000"`. It is a newtype wrapper around String (see `types.rs:DoogatId`). A custom `Deserialize` implementation accepts both YAML integer and string representations for backward compatibility with older doogats whose IDs were serialized as bare numbers.

### Three-zone Markdown

Every doogat file is divided into three zones separated by YAML front-matter fences (`---`):

1. **Frontmatter** -- YAML key-value pairs: id, title, date, type, tags, and arbitrary extra fields captured in a `BTreeMap<String, Value>`.
2. **Body** -- Free-form Markdown content below the closing `---` of the frontmatter.
3. **References** -- An optional third zone below a second `---` fence, used for structured references, parent links, and other relational metadata.

The parser splits these zones via `parser::split_zones()`, which returns a `Doogat` struct holding the three raw strings.

### ParsedDoogat

The fully parsed representation after extracting metadata from all three zones (see `types.rs:ParsedDoogat`). It holds:

- `meta` -- a `DoogatMeta` with id, title, date, type, tags, and extra fields.
- `body` -- the raw body text.
- `sections` -- parsed heading/content pairs from the body.
- `reference_section` -- the raw reference zone text.
- `inline_fields` -- Dataview-style `key:: value` pairs extracted from body and reference zones.
- `links` -- all links found across all zones, each tagged with a `LinkKind` and `Zone`.
- `body_tags` -- hashtags found in the body text (distinct from frontmatter tags).
- `checkboxes` -- task items with state (open/done/info), dates, and nesting level.
- `path` -- the Git-relative file path.

### DoogatMeta

Core metadata deserialized from YAML frontmatter. All fields are optional. The `extra` map captures arbitrary YAML fields not in the core schema, preserving them through parse/serialize round-trips. The `attachments` key in `extra` is reserved for the attachment system.

### Link and LinkKind

The parser recognizes four link syntaxes, each represented by a `LinkKind` variant:

- **WikiLink** -- `[[target|display]]`
- **Embed** -- `![[file#section|display]]`
- **MarkdownLink** -- `[title](url)`
- **BareUrl** -- raw `https://example.com` URLs

Each extracted `Link` records its target, optional display text, optional section anchor, kind, and the zone it was found in.

### Storage paths

Doogats are stored at `ddb/{id}.md`. Type definitions live at `ddb/_typedef/{id}.md`. When a typedef has `folder: true`, instances of that type are stored in a subdirectory: `ddb/{type_name}/{id}.md`. Binary attachments live under `reference/{doogat_id}/`.

### Repository layout

After `ddb init`, the on-disk layout is:

- `.git/ddb-node` -- local node UUID (gitignored)
- `ddb/` -- doogat Markdown files
- `ddb/_typedef/` -- type definition doogats
- `reference/` -- binary attachment files
- `.nodes/` -- node registry TOML files (git-tracked)
- `.crdt/temp/` -- temporary CRDT files for conflict resolution
- `.ddb/` -- local state (gitignored), contains `index.db` (SQLite)
- `.ddb.toml` -- repository configuration (compaction, CRDT, maintenance settings)
- `.ddb-version` -- format version number

### Other domain types

- `CommitHash` -- a String newtype wrapping a Git commit OID. Access the inner string via `.0`.
- `MergeResult` -- the outcome of a Git merge: `AlreadyUpToDate`, `FastForward`, `Clean`, or `Conflicts` (carrying a list of `ConflictFile` structs plus the theirs OID).
- `ConflictFile` -- a file in conflict with ancestor/ours/theirs content and optional HLC timestamps.
- `ResolvedFile` -- the result of CRDT resolution: path, merged content, and optional serialized CRDT bytes.
- `TableSchema` and `ColumnDef` -- schema metadata for materialized SQLite tables derived from typedef doogats.
- `Value` -- a domain-level value enum (String, Number, Bool, List, Map), decoupled from serde_yaml.
- `NodeConfig` -- per-device registration with UUID, name, known heads, HLC, and lifecycle status.
- `RepoConfig` -- repository settings for compaction, CRDT strategy, and maintenance auto-trigger.


## 3. Data Flow

Six primary paths move data through the system. Each path touches a specific subset of modules.

### Create

1. The CLI or server generates a new `DoogatId` from the current timestamp (see `parser::new_id()`).
2. The caller builds a `DoogatMeta` and body content.
3. `parser::serialize()` assembles the three-zone Markdown string.
4. `git_ops::GitRepo::commit_file()` writes the file into the Git working tree, stages it, and creates a commit.
5. `indexer::Index::index_doogat()` upserts the parsed doogat into SQLite (doogats, tags, fields, links, checkboxes, FTS5).
6. If running under the server, the actor emits a `Created` event on the event bus for WebSocket subscribers.

### Read

1. The caller provides a doogat ID.
2. `indexer::Index::resolve_path()` maps the ID to a Git-relative path (handling both flat and folder-typed layouts).
3. `git_ops::GitRepo::read_file()` reads the file content from the Git HEAD tree via libgit2, without touching the working directory.
4. `parser::parse()` splits zones and extracts all metadata, returning a `ParsedDoogat`.

### Update

1. Read the existing doogat (same as the Read path).
2. Modify the `ParsedDoogat` fields as requested.
3. `parser::serialize()` reassembles the three-zone Markdown.
4. `git_ops::GitRepo::commit_file()` writes and commits the updated content.
5. `indexer::Index::index_doogat()` re-upserts the doogat, replacing all prior index entries for that ID.
6. The server actor emits an `Updated` event.

### Search

1. `indexer::Index::ensure_fresh()` compares the stored HEAD OID in `_ddb_meta` against the actual Git HEAD. If they differ, a targeted incremental reindex runs for changed paths only (see `indexer/mod.rs:incremental_reindex()`).
2. The FTS5 virtual table `_ddb_fts` is queried with `MATCH` using porter stemming and unicode61 tokenization.
3. Results are ranked by BM25 score and returned with highlighted snippets.
4. `search_paginated()` adds limit/offset support and a total count.
5. `search_query::normalize()` parses the query string into an AST and serializes it to a canonical form (lowercased, sorted AND operands, explicit AND). The server exposes this as `queryNormalized` on SearchConnection and `normalizeSearchQuery` standalone query.

### Sync

1. `sync_manager::SyncManager::sync()` fetches from the remote via `git_ops::GitRepo::fetch()`.
2. `git_ops::GitRepo::merge_remote()` attempts a merge. Three outcomes:
   - **Fast-forward** or **clean merge** -- commit directly and update the index.
   - **Conflicts** -- collect `ConflictFile` structs for each conflicted path.
3. Conflicts are partitioned into three buckets: **delete-vs-edit** (one side empty), **add-add** (no ancestor, both sides non-empty), and **normal** (everything else). Each bucket has its own resolution strategy.
4. For add-add collisions, the later HLC wins; the loser is stashed for post-merge ID reassignment. Delete-vs-edit conflicts resurrect the surviving edit. Normal conflicts go through `crdt_resolver::resolve_conflicts()` (see Module Connections below).
5. All resolved files are committed in a single merge commit.
6. Post-merge: collision losers get new IDs, wikilinks are rewritten across the tree, and each reassignment is committed atomically.
7. The HLC is updated via `Hlc::recv()` to maintain causal ordering.
8. `indexer::Index::rebuild()` or an incremental reindex refreshes the SQLite index.
9. Node known-heads and last-sync timestamps are updated in `.nodes/{uuid}.toml`.

### SQL

1. `sql_engine::SqlEngine::execute()` parses the SQL string using `sqlparser`.
2. DDL statements (CREATE TABLE, ALTER TABLE, DROP TABLE) are translated into typedef doogat operations:
   - CREATE TABLE creates a `_typedef` doogat with column definitions, then materializes a SQLite table.
   - ALTER TABLE modifies the typedef doogat and re-materializes.
   - DROP TABLE deletes the typedef doogat and drops the materialized table.
3. DML statements are translated into doogat CRUD:
   - INSERT creates a new doogat with the specified type and field values.
   - UPDATE reads the existing doogat, modifies fields, and commits.
   - DELETE removes the doogat file and index entry.
4. SELECT statements run directly against the SQLite index (both core tables and materialized type tables).
5. Multi-statement transactions use a `TransactionBuffer` to batch writes and deletes into a single Git commit.

### Rename

1. `git_ops::GitRepo::rename_doogat()` moves the file to a new path and commits.
2. `parser::rewrite_links()` scans all other doogats for links pointing at the old path and rewrites them in a single batch commit.
3. The indexer is updated for both the renamed doogat and all doogats whose links changed.
4. A `RenameReport` lists updated files and any unresolvable references.


## 4. Module Connections

### parser to indexer

The parser extracts structured data from raw Markdown; the indexer consumes it. When `indexer::Index::index_doogat()` receives a `ParsedDoogat`, it upserts rows into `doogats`, `_ddb_tags`, `_ddb_fields`, `_ddb_links`, `_ddb_aliases`, `_ddb_checkboxes`, and `_ddb_attachments`. The FTS5 table `_ddb_fts` is kept in sync with the `doogats` table. The parser extracts four link kinds (wikilink, embed, markdown, bare URL), each stored with its zone and kind discriminant in `_ddb_links`.

### git_ops and crdt_resolver

When `git_ops::GitRepo::merge_remote()` detects conflicts, it returns `MergeResult::Conflicts` with a list of `ConflictFile` structs, each carrying ancestor/ours/theirs content and optional HLC timestamps. The CRDT resolver then handles each file:

1. `parser::split_zones()` splits all three versions into frontmatter/body/reference zones.
2. `merge_frontmatter()` uses an Automerge Map CRDT for field-level merge of YAML keys.
3. `merge_body()` uses an Automerge Text CRDT for character-level merge of body content.
4. `merge_reference()` uses an Automerge List CRDT, sorting entries on export.
5. The merged zones are reassembled via the parser into a final Markdown string.

Three CRDT strategy presets are supported (see `crdt_resolver.rs:resolve_conflicts()`):

- `preset:default` -- zone-level merge as described above.
- `preset:last-writer-wins` -- HLC comparison picks the newer version wholesale.
- `preset:append-log` -- bodies are concatenated chronologically, useful for project log doogats.

### sql_engine to git_ops

The SQL engine translates relational operations into doogat file operations. DDL (CREATE TABLE, ALTER TABLE, DROP TABLE) creates, modifies, or deletes `_typedef` doogats in `ddb/_typedef/`. DML (INSERT, UPDATE, DELETE) becomes `git_ops::GitRepo::commit_file()` or `commit_batch()` calls. SELECT goes directly to the SQLite index. The engine holds references to both `&Index` and `&dyn DoogatStore`, the latter satisfied by `GitRepo` which implements both `DoogatSource` and `DoogatStore`.

### Server actor bridge

The server uses an actor pattern to bridge async axum handlers with the synchronous core library (see `actor.rs:ActorHandle`). An `mpsc` channel carries `ActorCommand` variants to a background thread. Each command includes a `oneshot` sender for the response. The actor thread owns a `DoogatService` instance, delegating all operations to it. This ensures consistent behavior (NoSQL dual-writes, index freshness) across CLI, FFI, and server entry points.

For read-heavy workloads, a `ReadPool` (see `read_pool.rs`) provides concurrent read-only access. Each read acquires a semaphore permit and runs on `spawn_blocking` with a fresh `DoogatService` instance, bypassing the single-writer actor entirely. The pool size is configurable via server config.

### Indexer rebuild pipeline

Full rebuilds (see `indexer/mod.rs:rebuild()`) use a parallel pipeline powered by rayon:

1. `DoogatSource::list_doogats()` collects all doogat paths.
2. `DoogatSource::read_files_batch()` reads file contents (the default implementation is sequential; `GitRepo` can batch).
3. `parser::parse()` runs in parallel across files via rayon's `par_iter()`.
4. Parsed doogats are upserted into SQLite within a transaction.
5. Type definitions are collected and used to materialize typed tables.
6. `RebuildReport` tallies indexed count, materialized tables, inferred types, and any consistency warnings.

Incremental reindex (see `indexer/mod.rs:incremental_reindex()`) uses `DoogatSource::diff_paths()` to find changed files since the last known HEAD, then parses and upserts only those files.

### Event bus and subscriptions

The server's `EventBus` (see `events.rs`) is a tokio broadcast channel. After each successful mutation, the actor publishes a `DoogatEvent` with the kind (created/updated/deleted), doogat ID, type, and timestamp. GraphQL subscriptions and WebSocket connections subscribe to this channel for real-time updates.

### Hot schema reload

When a typedef doogat is created, updated, or deleted, the actor triggers a schema reload via `SchemaReloader` (see `reload.rs`). The reloader fetches current type schemas from the actor, rebuilds the dynamic GraphQL schema, and atomically swaps it into the `ArcSwap<Schema>` shared with all request handlers. This allows the GraphQL API to reflect typedef changes without a server restart.


## 5. Key Types and Traits

### Foundation types

- **DoogatId** -- 14-digit timestamp string newtype. See `types.rs:DoogatId`.
- **CommitHash** -- Git commit OID string newtype. Access the inner string via `.0`. See `types.rs:CommitHash`.
- **Value** -- Domain-level value enum (String, Number, Bool, List, Map), decoupled from serde_yaml. See `types.rs:Value`.
- **Zone** -- Enum identifying which part of the doogat data came from: Frontmatter, Body, or Reference. See `types.rs:Zone`.

### Doogat types

- **Doogat** -- Raw three-zone split before metadata extraction: `raw_frontmatter`, `body`, `reference_section`. See `types.rs:Doogat`.
- **DoogatMeta** -- Core metadata from YAML frontmatter with an `extra` BTreeMap for arbitrary fields. See `types.rs:DoogatMeta`.
- **ParsedDoogat** -- Full parsed representation with all extracted metadata, links, inline fields, sections, checkboxes, and body tags. See `types.rs:ParsedDoogat`.
- **InlineField** -- A Dataview-style `key:: value` pair with its source zone. See `types.rs:InlineField`.
- **Link** -- An extracted reference with target, display text, section anchor, kind, and zone. See `types.rs:Link`.
- **LinkKind** -- Discriminant for link syntax: WikiLink, MarkdownLink, Embed, BareUrl. See `types.rs:LinkKind`.
- **Section** -- A heading/content pair parsed from the body. See `types.rs:Section`.
- **CheckboxItem** -- A task item with state, content, dates, line number, and indent level. See `types.rs:CheckboxItem`.

### Sync and merge types

- **NodeConfig** -- Per-device registration stored in `.nodes/`. Tracks UUID, name, known heads, last sync time, HLC, and lifecycle status (Active/Stale/Retired). See `types.rs:NodeConfig`.
- **MergeResult** -- Outcome of `git_ops::merge_remote()`: AlreadyUpToDate, FastForward, Clean, or Conflicts. See `types.rs:MergeResult`.
- **ConflictFile** -- All three versions of a conflicted file plus optional HLC timestamps. See `types.rs:ConflictFile`.
- **ResolvedFile** -- Merged content after CRDT resolution, with optional serialized CRDT bytes. See `types.rs:ResolvedFile`.
- **SyncReport** -- Summary of a sync operation: direction, commits transferred, conflicts resolved, resurrected files, and collisions reassigned. See `types.rs:SyncReport`.

### Schema types

- **TableSchema** -- Schema for a materialized SQLite table: name, columns, CRDT strategy, template sections, and folder flag. See `types.rs:TableSchema`.
- **ColumnDef** -- A column definition with name, data type, optional FK reference, zone mapping, required flag, search boost, allowed values, and default. See `types.rs:ColumnDef`.

### Report types

- **RebuildReport** -- Rebuild statistics: indexed count, tables materialized, types inferred, and consistency warnings. See `types.rs:RebuildReport`.
- **CompactionReport** -- Compaction statistics: files removed, CRDT docs compacted, gc success, size metrics, and backup path. See `types.rs:CompactionReport`.
- **MaintenanceReport** -- Git maintenance results: tasks run, success flag, duration, and fallback indicator. See `types.rs:MaintenanceReport`.
- **RenameReport** -- Rename results: updated file list and unresolvable references. See `types.rs:RenameReport`.
- **SearchResult** and **PaginatedSearchResult** -- Search hits with id, title, path, snippet, rank, and pagination metadata. See `types.rs:SearchResult`.

### Error handling

`DoogatError` is a single thiserror-derived enum covering all failure modes: Git, Yaml, Sql, Automerge, Io, Toml, Parse, NotFound, Validation, InvalidPath, SqlEngine, and VersionMismatch. An optional `Redb` variant exists behind the `nosql` feature gate. See `error.rs:DoogatError`.

The crate defines a `Result<T>` alias as `std::result::Result<T, DoogatError>`. Every public function in the library returns this type. No panics in library code; all errors propagate via `?`.

### Core traits

Five traits define the module boundaries (see `traits.rs`):

- **DoogatSource** -- Read-only access to doogat storage: `list_doogats()`, `read_file()`, `head_oid()`, `diff_paths()`, and `read_files_batch()`. Implemented by `GitRepo`. The batch read has a default sequential implementation that concrete types can override.
- **DoogatStore** -- Read-write access extending DoogatSource: `commit_file()`, `commit_files()`, `delete_file()`, `delete_files()`, and `commit_batch()`. Implemented by `GitRepo`.
- **GitBackend** -- Full git backend abstraction extending DoogatSource + DoogatStore with remote ops, merge ops, binary file ops, commit introspection, and history queries. All OIDs are passed as `&str` hex strings (no `git2` types in the trait). Desktop-only hooks (commit-graph write, session counters) have default no-op implementations. Implemented by `GitRepo`. Enables swapping libgit2 for gitoxide per-feature.
- **DoogatIndex** -- Query and mutation operations on the search index: `index_doogat()`, `remove_doogat()`, `search()`, `search_paginated()`, `resolve_path()`, `query_raw()`, `find_typedef_path()`, and `execute_sql()`. Implemented by `Index`.
- **ConflictResolver** -- CRDT-based conflict resolution: `resolve_conflicts()` takes a list of `ConflictFile` structs and an optional strategy string, returning `ResolvedFile` results. The free function `crdt_resolver::resolve_conflicts()` implements this logic.

A `MockSource` in `traits::mock` provides an in-memory `DoogatSource` for unit tests.

### Hybrid Logical Clock

The `Hlc` struct (see `hlc.rs:Hlc`) combines a wall-clock millisecond timestamp, a logical counter, and a truncated node ID (first 8 characters of the UUID). Two operations maintain causality:

- `Hlc::now()` -- tick the clock for a local event, advancing the wall time or bumping the counter.
- `Hlc::recv()` -- merge on receive, taking the max of local, remote, and wall time, then bumping the counter on ties.

HLC values are totally ordered and used by the last-writer-wins CRDT strategy to determine which version survives a conflict.


## 6. Extension Points

### UniFFI facade

The FFI module (see `ffi.rs`) exposes a `DoogatDriver` struct that wraps `GitRepo`, `Index`, and `SqlEngine` behind a `Mutex`. It provides a UniFFI-friendly API with FFI-safe error types (`DdbError`) and value types (`SearchResult`, etc.) that mirror the core types but use only UniFFI-compatible primitives. Swift and Kotlin bindings are generated from the proc-macro annotations via the isolated `ddb-uniffi-bindgen` crate.

### Bundled types

The `bundled_types` module (see `bundled_types.rs`) ships predefined typedef templates as embedded Markdown strings. Available templates include `project` (with columns for completed, deliverable, parent, ticket, and body template sections for Description/Log/Plan/Solution), `contact` (aliases, contact-type, email, with folder storage), and `literature-note`. These are installed via `ddb type install <name>`, which writes the typedef doogat into `ddb/_typedef/`.

### Dynamic GraphQL schema

The server reads all `_typedef` doogats at startup via the actor, then builds a dynamic async-graphql schema (see `schema/mod.rs:build_schema()`). Each typedef generates a GraphQL object type with fields matching the column definitions, plus standard doogat fields (id, title, body, tags, links, etc.). The schema includes queries for listing, filtering, searching, and counting typed doogats, plus mutations for CRUD and SQL execution. Subscriptions use the event bus for real-time push.

When a typedef mutation occurs, the `SchemaReloader` rebuilds and atomically swaps in the new schema via `ArcSwap`, so clients see updated types without a server restart.

### PgWire protocol

The server exposes a Postgres wire protocol endpoint (see `pgwire.rs`) that allows standard SQL clients (psql, DBeaver, etc.) to connect and run queries against the SQLite index. SELECT statements are routed to the read pool; DDL/DML goes through the actor. MD5 password authentication uses the same bearer token as the HTTP API.

### REST API

A JSON REST API (see `rest.rs`) provides conventional CRUD endpoints alongside the GraphQL interface. Routes follow the pattern `/rest/doogats`, `/rest/doogats/:id`, with query parameters for filtering and pagination.

### WebSocket subscriptions

The WebSocket endpoint at `/ws` (see `ws.rs`) supports GraphQL subscriptions over the graphql-ws protocol. Authentication can occur either via the HTTP `Authorization` header during upgrade or via the `connection_init` payload for browser clients. Keepalive pings maintain connection health.

### Attachments

The attachment system (see `attachments.rs`) manages binary files associated with doogats. Files are stored in `reference/{doogat_id}/` and tracked in the doogat's frontmatter `attachments` array. Operations include `attach()` (validates the filename, detects MIME type, writes the blob, updates frontmatter, and commits), `detach()` (removes the blob and frontmatter entry), and `list()` (reads from frontmatter). The server provides a direct file-serving endpoint at `/attachments/{doogat_id}/{filename}` with path-traversal protection.

### Bundles

The bundle system (see `bundle.rs`) supports air-gapped sync between devices that cannot reach each other over a network. Bundles are tar archives containing a Git bundle (delta or full), node registration files, a manifest, and a SHA-256 checksum. Export creates a bundle targeting a specific node (delta based on that node's known heads) or a full bundle for backup. Import applies the bundle to the local repository and updates sync state.

### Maintenance

The maintenance module (see `maintenance.rs`) wraps `git maintenance run` for repository housekeeping (commit-graph, loose-objects, incremental-repack, pack-refs). If the system `git` binary lacks the `maintenance` subcommand, it falls back to `git gc --auto`. Auto-triggering can be configured in `.ddb.toml` with a write-count threshold. The server optionally runs maintenance on a periodic timer.

### Compaction

The compaction module (see `compaction/mod.rs`) cleans up CRDT temporary files in `.crdt/temp/` and runs Git garbage collection. It computes a shared-head across all active nodes' known heads to determine which CRDT documents are safe to prune. A pre-compaction backup bundle can be created automatically. The `compact()` function reports bytes reclaimed, files removed, and CRDT documents compacted.


## 7. Testing Strategy

Doogat DB uses a layered testing approach with six distinct tiers, each covering different aspects of the system.

### Unit tests

In-module `#[cfg(test)] mod tests` blocks test individual functions in isolation. These run with `cargo test` (the fast local tier) and cover parsing edge cases, CRDT merge logic, SQL translation, type inference, HLC arithmetic, and error mapping. The `MockSource` in `traits::mock` provides an in-memory doogat store for tests that need a `DoogatSource` without touching Git.

### Integration tests

Tests in `ddb-core/tests/` exercise multi-module interactions: sync between two repos, property-based tests for parse/serialize round-trips, and CRDT resolution across different conflict scenarios. Run with `cargo test -p ddb-core`.

### Property tests

Proptest-based tests in `ddb-core/tests/property_tests.rs` generate random doogat content and verify invariants like parse/serialize round-trip fidelity and CRDT merge commutativity. The default case count is suitable for CI; a thorough run (`PROPTEST_CASES=5000`) takes around 20 minutes.

### End-to-end tests

The `tests/e2e/` directory contains assert_cmd-based tests that exercise the `ddb` binary as a black box. These tests create temporary repositories, run CLI commands, and assert on stdout/stderr output and exit codes. Server tests use reqwest to hit the HTTP endpoints. The binary must be built first: `cargo build -p ddb-cli && cargo test -p ddb-e2e`.

### Smoke tests

`tests/smoke.sh` (bash) and `tests/smoke.ps1` (PowerShell) provide quick CLI validation (init, CRUD, search, SQL, types, compact). `tests/integration.sh` and `tests/integration.ps1` run the smoke tests first, then continue with server, sync, CRDT conflict resolution, bundles, and advanced SQL tests. All files follow a numbered-section pattern with a `pass` helper for status reporting.

### Benchmarks

Criterion benchmarks in `ddb-core/benches/` measure CRUD operations and search performance at 1K doogats. `crud.rs` benchmarks create, read, update, and delete cycles. `search.rs` benchmarks FTS5 search, SQL SELECT, and full reindex. Run with `cargo bench`; compile-only with `cargo bench --no-run`.

### Full suite

`cargo test --workspace` (aliased as `cargo test-full`) runs all of the above: unit tests, integration tests, and e2e tests across all crates. This is the definitive validation before merging any change.

## 8. Consistency Auto-Fix

The `consistency` module (`consistency/mod.rs`) provides a detect-then-apply pipeline for normalizing doogats. The `ddb fix` command scans all doogats and corrects common data quality issues in a single atomic commit.

### Detection

`detect_fixes(parsed, schema)` inspects a `ParsedDoogat` and returns a `Vec<Fix>` with severity-ordered issues:

- **Error**: cross-zone duplicate fields (same key in frontmatter and body inline fields)
- **Warning**: missing type default, missing title (derived from H1 or filename), title doesn't match typedef `title_template`
- **Info**: duplicate tags, unsorted tags, `#`-prefixed tags, non-kebab-case keys, untrimmed/uncapitalized title, H1-title mismatch

### Application

`apply_fixes(parsed, fixes)` modifies the `ParsedDoogat` in-place and re-serializes via `parser::serialize()`. Tag fixes run in order: strip hash, dedup, sort. Key normalization uses `to_kebab_case()` which handles CamelCase, snake_case, and acronyms (e.g. `XMLParser` -> `xml-parser`).

### Orchestration

`fix_all(repo, index, dry_run)` iterates all doogats, detects fixes per typedef schema, applies them, and commits atomically. Dry-run mode collects the report without modifying files.

### Migration

`migrate_all(repo, dry_run)` runs versioned field-level migrations: `zkn-id` -> `id`, `tag` (singular) -> `tags`, type normalization (`loop` -> `project`, `doogat`/`wiki-article` -> `note`). Version tracked in `.ddb/migration-version`. Invoked via `ddb fix --migrate`.

### Zone migration

`zone_migrate_all(repo, index, dry_run)` compares each column's current zone (frontmatter, body, or reference) against the typedef's `effective_zone()` and rewrites the doogat to move data to the correct zone. For example, if a column was changed from body to frontmatter via `ALTER TABLE ... SET ZONE frontmatter FOR ...`, zone migration extracts the `## column_name` body section and places the value in frontmatter YAML. Also invoked via `ddb fix --migrate`.

### CLI

```
ddb fix [--dry-run] [-v/--verbose] [--migrate]
```

## 9. Discovery

The discovery system surfaces latent connections, maintenance issues, and knowledge gaps across the doogat graph. Six queries are available: four via both CLI and GraphQL, two (recent changes, link density) via CLI only.

### Unlinked mentions

Finds doogats whose body text mentions another doogat's title without linking to it. Uses FTS5 phrase matching against the target's title, then excludes doogats that already link to the target (by path, ID, or alias). Self-references are also excluded.

```bash
ddb discover mentions <id>
ddb discover mentions --all
```

```graphql
query { unlinkedMentions(id: "20260301120000") { sourceId sourceTitle snippet } }
```

### Link suggestions

Suggests related doogats based on a hybrid scoring algorithm. Candidates with shared tags are scored by Jaccard similarity (weighted 0.6), then FTS5 BM25 content similarity against the source title is added (weighted 0.4). Already-linked doogats are excluded. When the source has no tags, the system falls back to content-only scoring.

```bash
ddb discover similar <id> [--limit N]
```

```graphql
query { suggestions(id: "20260301120000", limit: 5) { id title score sharedTags } }
```

### Staleness tracking

Identifies doogats that haven't been updated within their type's configured threshold. Types opt in by setting `stale_after_days` in the typedef frontmatter:

```yaml
stale_after_days: 30
```

The last-updated date uses a priority chain: git revision date > frontmatter `date` field > indexer `updated_at`. Results are sorted by staleness (most stale first).

```bash
ddb discover stale [--type <type>]
```

```graphql
query { staleDoogats(type: "project") { id title doogatType lastUpdated dateSource daysStale thresholdDays } }
```

### Orphan detection

Finds doogats with zero incoming links (no other doogat links to them). Typedef doogats are excluded since they are structural, not content nodes. Results include the outgoing link count to help assess whether an orphan is isolated or simply unreferenced.

```bash
ddb discover orphans [--type <type>]
```

```graphql
query { orphanDoogats(type: "note") { id title doogatType outgoingLinks } }
```

### Recent changes

Lists doogats modified within a configurable time window, sorted by recency. Uses the frontmatter `date` field as the primary date source, falling back to the indexer's `updated_at` timestamp when no frontmatter date exists. Typedefs are excluded.

```bash
ddb discover recent [--days N] [--type-filter <type>]
```

Default lookback is 7 days. Output columns: id, title, type, last_modified.

### Link density

Reports inbound and outbound link counts per doogat, sorted by total density (inbound + outbound) descending. Surfaces hub doogats with many connections and identifies isolated nodes with zero links. Typedefs are excluded.

```bash
ddb discover link-density [--type-filter <type>]
```

Output columns: id, title, type, inbound count, outbound count, density score.


## 10. Metadata Path Navigation

The `Value` enum supports nested structures (`Map`, `List`), and path navigation lets you traverse them using dot/bracket notation.

### Path syntax

- `author.name` -- navigate nested maps
- `tags[0]` -- index into lists
- `author.address.city` -- arbitrary depth
- `a[0].b.c[2]` -- mixed map/list paths
- `a\.b` -- escaped dot (literal dot in key name)

### Core API (types.rs)

```rust
// Navigate
value.get_path("author.name")?;        // -> &Value

// Mutate
value.set_path("author.name", val)?;   // creates intermediate maps
value.remove_path("author.age")?;      // returns removed value

// Type-safe accessors
value.str_at("author.name")?;          // -> &str
value.f64_at("score")?;                // -> f64
value.bool_at("active")?;              // -> bool
value.list_at("tags")?;                // -> &[Value]
value.map_at("author")?;               // -> &BTreeMap<String, Value>
```

Errors are structured as `PathError` with variants `KeyNotFound`, `IndexOutOfBounds`, `TypeMismatch`, and `InvalidPath`, each carrying the full path and failing segment context.

### Indexer integration

Nested `Map` and `List` values in frontmatter `extra` fields are flattened into the `_ddb_fields` table with dot/bracket notation keys:

```yaml
author:
  name: Alice
  email: alice@example.com
scores:
  - 10
  - 20
```

Produces `_ddb_fields` rows with keys: `author.name`, `author.email`, `scores[0]`, `scores[1]`.

### SQL and GraphQL

Column names containing `.` or `[` trigger path navigation in both:
- SQL engine `extract_column_value` (materialized type views)
- GraphQL `extract_typed_field` (dynamic schema resolvers)

## 12. Sequence Navigation

Doogats can form ordered chains using the `sequence` frontmatter field. A doogat points to its parent:

```yaml
---
id: 20260315120002
title: Chapter 3
sequence: 20260315120000
---
```

Children are discovered by reverse lookup: all doogats where `sequence == this.id`. No schema change is needed — the existing `_ddb_fields` table stores the `sequence` key.

### Core queries (indexer/)

- `sequence_children(id)` — direct children sorted by ID (chronological)
- `sequence_breadcrumb(id)` — walk up parent chain to root, return root-to-self path
- `sequence_info(id)` — parent + children + breadcrumb in one call
- `broken_sequences()` — LEFT JOIN to find sequence fields referencing non-existent parents

Cycle detection: breadcrumb walk tracks visited IDs and breaks after 100 iterations.

### MOC (Map of Content) recognition

Doogats with `role: moc` (or `role: index`, `role: hub`, `role: structure`) in frontmatter are recognized as structural organizers — natural sequence roots. No special code is needed: the `role` field is stored in `_ddb_fields` like any other frontmatter extra, so these doogats are discoverable via SQL queries:

```sql
SELECT z.id, z.title FROM _ddb_fields f
JOIN doogats z ON z.id = f.doogat_id
WHERE f.key = 'role' AND f.value = 'moc'
```

A MOC doogat typically has no `sequence` field (it's the root) and its children point to it via their `sequence` field.

### CLI

```bash
ddb sequence tree <id>         # breadcrumb line + children list
ddb sequence breadcrumb <id>   # root → ... → self
ddb sequence broken            # broken sequence references
```

### GraphQL

```graphql
sequenceInfo(id: ID!): SequenceInfo!
sequenceChildren(id: ID!): [SequenceNode!]!
sequenceBreadcrumb(id: ID!): [SequenceNode!]!
brokenSequences: [BrokenSequence!]!
```

## 13. Zone Inference

When `CREATE TABLE` defines columns, each column's SQL data type determines its default zone placement. This inference runs in `sql_engine/mod.rs:extract_columns()` during typedef creation.

### Inference rules (applied in order)

1. If column has a `REFERENCES` clause, zone is **Reference**.
2. If data type is numeric (`INTEGER`, `REAL`, `BOOLEAN`), zone is **Frontmatter**.
3. If data type is a short string, zone is **Frontmatter**:
   - `CHAR`, `CHARACTER`, `TINYTEXT`
   - `VARCHAR`/`CHAR VARYING` with length <= 255 (or no length)
   - `ENUM`, `SET`
4. Otherwise (`TEXT`, `MEDIUMTEXT`, `LONGTEXT`), zone is **Body**.

```rust
let zone = if references.is_some() {
    Some(Zone::Reference)
} else if is_numeric_type(&data_type) || is_short_string_type(&col.data_type) {
    Some(Zone::Frontmatter)
} else {
    Some(Zone::Body)
};
```

Zone overrides via `ALTER TABLE ... SET ZONE` change the typedef's column zone after creation.


## 14. Pre-Parse Interception

The SQL engine supports custom DDL extensions that `sqlparser` cannot parse: `ALTER TABLE SET ZONE`, `SET TITLE TEMPLATE`, and `DROP TITLE TEMPLATE`. These are intercepted before the SQL reaches the parser.

### Flow

1. `execute(sql)` calls `try_custom_ddl(sql)` first.
2. Three `OnceLock<Regex>` patterns match the custom syntax:
   - `ALTER TABLE <t> SET ZONE <zone> FOR <col>`
   - `ALTER TABLE <t> SET TITLE TEMPLATE '<template>'`
   - `ALTER TABLE <t> DROP TITLE TEMPLATE`
3. If a regex matches, capture groups are extracted and dispatched to `handle_set_zone()` or `handle_title_template()`.
4. If no regex matches, `try_custom_ddl()` returns `None` and normal `sqlparser` flow continues.

This approach keeps the parser dependency clean while supporting Doogat DB-specific DDL.


## 15. Junction Tables

Multi-valued `REFERENCES` columns use auto-created junction tables for many-to-many relationships.

### Auto-creation (sql_engine/mod.rs)

When `CREATE TABLE` includes a column with `REFERENCES`, `handle_create_table()` calls `junction_table_ddl(table_name, col_name)` to generate:

```sql
CREATE TABLE IF NOT EXISTS "{table}_{col}" (
  "{table}_id" TEXT NOT NULL,
  "{col}_id" TEXT NOT NULL,
  PRIMARY KEY ("{table}_id", "{col}_id")
)
```

This junction table is created in SQLite alongside the main materialized table.

### Write-through (materialize.rs)

During doogat indexing, `materialize_row()` handles junction population:

1. Insert the main row into the type table.
2. For each column where `col.references.is_some()`, call `extract_multi_reference_values()`.
3. This function filters the doogat's `inline_fields` for entries where `f.key == col_name` and `f.zone == Zone::Reference`.
4. For each reference value, execute `INSERT OR IGNORE` into the junction table.
5. Before rematerialization, old junction rows are deleted with `DELETE FROM "{t}_{c}" WHERE "{t}_id" = ?`.

### DROP CASCADE

`DROP TABLE <name> CASCADE` drops both the main table and all its junction tables. The engine iterates columns with references and drops each `{table}_{col}` table before dropping the main table and deleting the typedef doogat.

### GraphQL list field resolution (schema/base_types.rs)

For each column with a `REFERENCES` target, the dynamic GraphQL schema adds a pluralized list field:

1. `build_typed_object_type()` calls `pluralize(&col.name)` to derive the field name (e.g., `category` -> `categories`).
2. The field is typed as `[String!]!` (non-null list of non-null strings).
3. At query time, `build_typed_object()` extracts all `inline_fields` matching the column name in `Zone::Reference` and returns them as a list.
4. Both singular (first value) and plural (all values) fields are available.

### Pluralization rules

- Ends with 's' -> add 'es' (e.g., `class` -> `classes`)
- Ends with 'y' -> replace 'y' with 'ies' (e.g., `category` -> `categories`)
- Otherwise -> add 's' (e.g., `assignee` -> `assignees`)


## 16. Help Guides

The CLI includes an embedded guide system via `ddb help <topic>`. When called without a topic, it lists available guides. Each guide is a prose walkthrough of a workflow (e.g., data modeling, zone configuration, API access).

The implementation lives in the `Command::Help` arm of `main.rs`. Guide text is returned inline (not loaded from files). Several subcommands include `after_help` or `after_long_help` hints that point users to relevant guides.

For deeper detail on any module, see the corresponding document in `docs/src/technical/`:

- `technical/parser.md` -- three-zone Markdown parsing
- `technical/git-ops.md` -- Git storage layer
- `technical/crdt-resolver.md` -- conflict resolution strategy
- `technical/indexer.md` -- SQLite index, FTS5, type inference
- `technical/sql-engine.md` -- SQL translation layer
- `technical/sync.md` -- multi-device sync protocol
- `technical/server.md` -- server architecture and API
- `technical/ffi.md` -- UniFFI bindings
- `technical/data-model.md` -- doogat format and frontmatter schema
- `technical/errors.md` -- error handling patterns
