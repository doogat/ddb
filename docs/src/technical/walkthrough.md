# ZettelDB Code Walkthrough

ZettelDB is a hybrid Git-CRDT decentralized Zettelkasten database written in Rust. Git is the source of truth for all data; a SQLite index with FTS5 provides fast reads and full-text search. When concurrent edits on different devices produce Git merge conflicts, an Automerge CRDT layer resolves them at the zone level. The system ships as a CLI, a multi-protocol server (GraphQL, REST, PgWire, WebSocket), and a UniFFI facade for native Swift/Kotlin apps.


## 1. Workspace Layout

The Cargo workspace lives at the repository root. Five members are declared in the root `Cargo.toml`; three of them are default members so that a bare `cargo build` compiles the fast local loop.

### zdb-core

All domain logic lives here: parsing, Git storage, CRDT conflict resolution, SQLite indexing, SQL translation, sync orchestration, compaction, attachments, and the UniFFI FFI facade. Every other crate depends on it. The crate root at `lib.rs` re-exports every public module and calls `uniffi::setup_scaffolding!()` to wire up FFI scaffolding. An optional `nosql` feature gate enables an experimental redb-backed key-value index.

### zdb-cli

A thin clap-derived binary (`zdb`) in a single `main.rs`. It wires CLI flags to core library calls. Subcommands cover the full lifecycle: `init`, `create`, `read`, `update`, `delete`, `search`, `query`, `sync`, `reindex`, `compact`, `rename`, `serve`, `type`, `node`, `bundle`, `attach`, `detach`, `attachments`, `get`, `scan`, `backlinks`, `maintenance`, and `update-bin`. An embedded `updater` module handles self-update from GitHub releases.

### zdb-server

An axum-based multi-protocol server library. Protocols: GraphQL (dynamic schema from typedef zettels), REST (JSON CRUD), PgWire (Postgres wire protocol for SQL clients), and WebSocket (GraphQL subscriptions). The crate wires up bearer-token auth, a single-writer actor, a read-only connection pool, an event bus for real-time subscriptions, and hot schema reload when typedef zettels change.

### zdb-uniffi-bindgen

An isolated binary crate whose sole purpose is to host the UniFFI bindgen tool. Keeping it separate avoids polluting zdb-core with binary targets and simplifies cross-compilation for Swift and Kotlin binding generation.

### tests (zdb-e2e)

An end-to-end test harness using `assert_cmd` for CLI tests and `reqwest` for server tests. E2E tests require the `zdb` binary on the PATH, so `cargo build -p zdb-cli` must run first. The crate lives in `tests/` and is declared as the fifth workspace member.

### Build aliases and test tiers

The workspace defines custom Cargo aliases:

- `cargo test` runs the fast local tier (unit tests in default members).
- `cargo test-ci` runs the bounded CI matrix (unit and binary targets only).
- `cargo test-full` runs the complete suite including workspace-wide and e2e tests.

Additional test surfaces include `tests/smoke.sh` (bash) and `tests/smoke.ps1` (PowerShell) for CLI smoke tests, and Criterion benchmarks in `zdb-core/benches/` for CRUD and search performance.


## 2. Core Data Model

All domain types are defined in `types.rs`. The data model is intentionally flat: every zettel is a Markdown file with structured frontmatter, and the system derives all relational structure from that content.

### ZettelId

A 14-digit timestamp string in the format `YYYYMMDDHHmmss`, for example `"20260226120000"`. It is a newtype wrapper around String (see `types.rs:ZettelId`). A custom `Deserialize` implementation accepts both YAML integer and string representations for backward compatibility with older zettels whose IDs were serialized as bare numbers.

### Three-zone Markdown

Every zettel file is divided into three zones separated by YAML front-matter fences (`---`):

1. **Frontmatter** -- YAML key-value pairs: id, title, date, type, tags, and arbitrary extra fields captured in a `BTreeMap<String, Value>`.
2. **Body** -- Free-form Markdown content below the closing `---` of the frontmatter.
3. **References** -- An optional third zone below a second `---` fence, used for structured references, parent links, and other relational metadata.

The parser splits these zones via `parser::split_zones()`, which returns a `Zettel` struct holding the three raw strings.

### ParsedZettel

The fully parsed representation after extracting metadata from all three zones (see `types.rs:ParsedZettel`). It holds:

- `meta` -- a `ZettelMeta` with id, title, date, type, tags, and extra fields.
- `body` -- the raw body text.
- `sections` -- parsed heading/content pairs from the body.
- `reference_section` -- the raw reference zone text.
- `inline_fields` -- Dataview-style `key:: value` pairs extracted from body and reference zones.
- `links` -- all links found across all zones, each tagged with a `LinkKind` and `Zone`.
- `body_tags` -- hashtags found in the body text (distinct from frontmatter tags).
- `checkboxes` -- task items with state (open/done/info), dates, and nesting level.
- `path` -- the Git-relative file path.

### ZettelMeta

Core metadata deserialized from YAML frontmatter. All fields are optional. The `extra` map captures arbitrary YAML fields not in the core schema, preserving them through parse/serialize round-trips. The `attachments` key in `extra` is reserved for the attachment system.

### Link and LinkKind

The parser recognizes four link syntaxes, each represented by a `LinkKind` variant:

- **WikiLink** -- `[[target|display]]`
- **Embed** -- `![[file#section|display]]`
- **MarkdownLink** -- `[title](url)`
- **BareUrl** -- raw `https://example.com` URLs

Each extracted `Link` records its target, optional display text, optional section anchor, kind, and the zone it was found in.

### Storage paths

Zettels are stored at `zettelkasten/{id}.md`. Type definitions live at `zettelkasten/_typedef/{id}.md`. When a typedef has `folder: true`, instances of that type are stored in a subdirectory: `zettelkasten/{type_name}/{id}.md`. Binary attachments live under `reference/{zettel_id}/`.

### Repository layout

After `zdb init`, the on-disk layout is:

- `.git/zdb-node` -- local node UUID (gitignored)
- `zettelkasten/` -- zettel Markdown files
- `zettelkasten/_typedef/` -- type definition zettels
- `reference/` -- binary attachment files
- `.nodes/` -- node registry TOML files (git-tracked)
- `.crdt/temp/` -- temporary CRDT files for conflict resolution
- `.zdb/` -- local state (gitignored), contains `index.db` (SQLite)
- `.zetteldb.toml` -- repository configuration (compaction, CRDT, maintenance settings)
- `.zetteldb-version` -- format version number

### Other domain types

- `CommitHash` -- a String newtype wrapping a Git commit OID. Access the inner string via `.0`.
- `MergeResult` -- the outcome of a Git merge: `AlreadyUpToDate`, `FastForward`, `Clean`, or `Conflicts` (carrying a list of `ConflictFile` structs plus the theirs OID).
- `ConflictFile` -- a file in conflict with ancestor/ours/theirs content and optional HLC timestamps.
- `ResolvedFile` -- the result of CRDT resolution: path, merged content, and optional serialized CRDT bytes.
- `TableSchema` and `ColumnDef` -- schema metadata for materialized SQLite tables derived from typedef zettels.
- `Value` -- a domain-level value enum (String, Number, Bool, List, Map), decoupled from serde_yaml.
- `NodeConfig` -- per-device registration with UUID, name, known heads, HLC, and lifecycle status.
- `RepoConfig` -- repository settings for compaction, CRDT strategy, and maintenance auto-trigger.


## 3. Data Flow

Six primary paths move data through the system. Each path touches a specific subset of modules.

### Create

1. The CLI or server generates a new `ZettelId` from the current timestamp (see `parser::new_id()`).
2. The caller builds a `ZettelMeta` and body content.
3. `parser::serialize()` assembles the three-zone Markdown string.
4. `git_ops::GitRepo::commit_file()` writes the file into the Git working tree, stages it, and creates a commit.
5. `indexer::Index::index_zettel()` upserts the parsed zettel into SQLite (zettels, tags, fields, links, checkboxes, FTS5).
6. If running under the server, the actor emits a `Created` event on the event bus for WebSocket subscribers.

### Read

1. The caller provides a zettel ID.
2. `indexer::Index::resolve_path()` maps the ID to a Git-relative path (handling both flat and folder-typed layouts).
3. `git_ops::GitRepo::read_file()` reads the file content from the Git HEAD tree via libgit2, without touching the working directory.
4. `parser::parse()` splits zones and extracts all metadata, returning a `ParsedZettel`.

### Update

1. Read the existing zettel (same as the Read path).
2. Modify the `ParsedZettel` fields as requested.
3. `parser::serialize()` reassembles the three-zone Markdown.
4. `git_ops::GitRepo::commit_file()` writes and commits the updated content.
5. `indexer::Index::index_zettel()` re-upserts the zettel, replacing all prior index entries for that ID.
6. The server actor emits an `Updated` event.

### Search

1. `indexer::Index::ensure_fresh()` compares the stored HEAD OID in `_zdb_meta` against the actual Git HEAD. If they differ, a targeted incremental reindex runs for changed paths only (see `indexer.rs:incremental_reindex()`).
2. The FTS5 virtual table `_zdb_fts` is queried with `MATCH` using porter stemming and unicode61 tokenization.
3. Results are ranked by BM25 score and returned with highlighted snippets.
4. `search_paginated()` adds limit/offset support and a total count.

### Sync

1. `sync_manager::SyncManager::sync()` fetches from the remote via `git_ops::GitRepo::fetch()`.
2. `git_ops::GitRepo::merge_remote()` attempts a merge. Three outcomes:
   - **Fast-forward** or **clean merge** -- commit directly and update the index.
   - **Conflicts** -- collect `ConflictFile` structs for each conflicted path.
3. For conflicts, `crdt_resolver::resolve_conflicts()` runs per-zone CRDT merge (see Module Connections below).
4. Resolved files are committed, and the sync manager pushes to the remote.
5. The HLC is updated via `Hlc::recv()` to maintain causal ordering.
6. `indexer::Index::rebuild()` or an incremental reindex refreshes the SQLite index.
7. Node known-heads and last-sync timestamps are updated in `.nodes/{uuid}.toml`.

### SQL

1. `sql_engine::SqlEngine::execute()` parses the SQL string using `sqlparser`.
2. DDL statements (CREATE TABLE, ALTER TABLE, DROP TABLE) are translated into typedef zettel operations:
   - CREATE TABLE creates a `_typedef` zettel with column definitions, then materializes a SQLite table.
   - ALTER TABLE modifies the typedef zettel and re-materializes.
   - DROP TABLE deletes the typedef zettel and drops the materialized table.
3. DML statements are translated into zettel CRUD:
   - INSERT creates a new zettel with the specified type and field values.
   - UPDATE reads the existing zettel, modifies fields, and commits.
   - DELETE removes the zettel file and index entry.
4. SELECT statements run directly against the SQLite index (both core tables and materialized type tables).
5. Multi-statement transactions use a `TransactionBuffer` to batch writes and deletes into a single Git commit.

### Rename

1. `git_ops::GitRepo::rename_zettel()` moves the file to a new path and commits.
2. `parser::rewrite_links()` scans all other zettels for links pointing at the old path and rewrites them in a single batch commit.
3. The indexer is updated for both the renamed zettel and all zettels whose links changed.
4. A `RenameReport` lists updated files and any unresolvable references.


## 4. Module Connections

### parser to indexer

The parser extracts structured data from raw Markdown; the indexer consumes it. When `indexer::Index::index_zettel()` receives a `ParsedZettel`, it upserts rows into `zettels`, `_zdb_tags`, `_zdb_fields`, `_zdb_links`, `_zdb_aliases`, `_zdb_checkboxes`, and `_zdb_attachments`. The FTS5 table `_zdb_fts` is kept in sync with the `zettels` table. The parser extracts four link kinds (wikilink, embed, markdown, bare URL), each stored with its zone and kind discriminant in `_zdb_links`.

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
- `preset:append-log` -- bodies are concatenated chronologically, useful for project log zettels.

### sql_engine to git_ops

The SQL engine translates relational operations into zettel file operations. DDL (CREATE TABLE, ALTER TABLE, DROP TABLE) creates, modifies, or deletes `_typedef` zettels in `zettelkasten/_typedef/`. DML (INSERT, UPDATE, DELETE) becomes `git_ops::GitRepo::commit_file()` or `commit_batch()` calls. SELECT goes directly to the SQLite index. The engine holds references to both `&Index` and `&dyn ZettelStore`, the latter satisfied by `GitRepo` which implements both `ZettelSource` and `ZettelStore`.

### Server actor bridge

The server uses an actor pattern to bridge async axum handlers with the synchronous core library (see `actor.rs:ActorHandle`). An `mpsc` channel carries `ActorCommand` variants to a background thread. Each command includes a `oneshot` sender for the response. The actor thread owns the `GitRepo`, `Index`, and `SqlEngine`, executing operations sequentially to maintain write consistency.

For read-heavy workloads, a `ReadPool` (see `read_pool.rs`) provides concurrent read-only access. Each read acquires a semaphore permit and runs on `spawn_blocking` with its own `Index` and `GitRepo` handles, bypassing the single-writer actor entirely. The pool size is configurable via server config.

### Indexer rebuild pipeline

Full rebuilds (see `indexer.rs:rebuild()`) use a parallel pipeline powered by rayon:

1. `ZettelSource::list_zettels()` collects all zettel paths.
2. `ZettelSource::read_files_batch()` reads file contents (the default implementation is sequential; `GitRepo` can batch).
3. `parser::parse()` runs in parallel across files via rayon's `par_iter()`.
4. Parsed zettels are upserted into SQLite within a transaction.
5. Type definitions are collected and used to materialize typed tables.
6. `RebuildReport` tallies indexed count, materialized tables, inferred types, and any consistency warnings.

Incremental reindex (see `indexer.rs:incremental_reindex()`) uses `ZettelSource::diff_paths()` to find changed files since the last known HEAD, then parses and upserts only those files.

### Event bus and subscriptions

The server's `EventBus` (see `events.rs`) is a tokio broadcast channel. After each successful mutation, the actor publishes a `ZettelEvent` with the kind (created/updated/deleted), zettel ID, type, and timestamp. GraphQL subscriptions and WebSocket connections subscribe to this channel for real-time updates.

### Hot schema reload

When a typedef zettel is created, updated, or deleted, the actor triggers a schema reload via `SchemaReloader` (see `reload.rs`). The reloader fetches current type schemas from the actor, rebuilds the dynamic GraphQL schema, and atomically swaps it into the `ArcSwap<Schema>` shared with all request handlers. This allows the GraphQL API to reflect typedef changes without a server restart.


## 5. Key Types and Traits

### Foundation types

- **ZettelId** -- 14-digit timestamp string newtype. See `types.rs:ZettelId`.
- **CommitHash** -- Git commit OID string newtype. Access the inner string via `.0`. See `types.rs:CommitHash`.
- **Value** -- Domain-level value enum (String, Number, Bool, List, Map), decoupled from serde_yaml. See `types.rs:Value`.
- **Zone** -- Enum identifying which part of the zettel data came from: Frontmatter, Body, or Reference. See `types.rs:Zone`.

### Zettel types

- **Zettel** -- Raw three-zone split before metadata extraction: `raw_frontmatter`, `body`, `reference_section`. See `types.rs:Zettel`.
- **ZettelMeta** -- Core metadata from YAML frontmatter with an `extra` BTreeMap for arbitrary fields. See `types.rs:ZettelMeta`.
- **ParsedZettel** -- Full parsed representation with all extracted metadata, links, inline fields, sections, checkboxes, and body tags. See `types.rs:ParsedZettel`.
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
- **SyncReport** -- Summary of a sync operation: direction, commits transferred, conflicts resolved, resurrected files. See `types.rs:SyncReport`.

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

`ZettelError` is a single thiserror-derived enum covering all failure modes: Git, Yaml, Sql, Automerge, Io, Toml, Parse, NotFound, Validation, InvalidPath, SqlEngine, and VersionMismatch. An optional `Redb` variant exists behind the `nosql` feature gate. See `error.rs:ZettelError`.

The crate defines a `Result<T>` alias as `std::result::Result<T, ZettelError>`. Every public function in the library returns this type. No panics in library code; all errors propagate via `?`.

### Core traits

Four traits define the module boundaries (see `traits.rs`):

- **ZettelSource** -- Read-only access to zettel storage: `list_zettels()`, `read_file()`, `head_oid()`, `diff_paths()`, and `read_files_batch()`. Implemented by `GitRepo`. The batch read has a default sequential implementation that concrete types can override.
- **ZettelStore** -- Read-write access extending ZettelSource: `commit_file()`, `commit_files()`, `delete_file()`, `delete_files()`, and `commit_batch()`. Implemented by `GitRepo`.
- **ZettelIndex** -- Query and mutation operations on the search index: `index_zettel()`, `remove_zettel()`, `search()`, `search_paginated()`, `resolve_path()`, `query_raw()`, `find_typedef_path()`, and `execute_sql()`. Implemented by `Index`.
- **ConflictResolver** -- CRDT-based conflict resolution: `resolve_conflicts()` takes a list of `ConflictFile` structs and an optional strategy string, returning `ResolvedFile` results. The free function `crdt_resolver::resolve_conflicts()` implements this logic.

A `MockSource` in `traits::mock` provides an in-memory `ZettelSource` for unit tests.

### Hybrid Logical Clock

The `Hlc` struct (see `hlc.rs:Hlc`) combines a wall-clock millisecond timestamp, a logical counter, and a truncated node ID (first 8 characters of the UUID). Two operations maintain causality:

- `Hlc::now()` -- tick the clock for a local event, advancing the wall time or bumping the counter.
- `Hlc::recv()` -- merge on receive, taking the max of local, remote, and wall time, then bumping the counter on ties.

HLC values are totally ordered and used by the last-writer-wins CRDT strategy to determine which version survives a conflict.


## 6. Extension Points

### UniFFI facade

The FFI module (see `ffi.rs`) exposes a `ZettelDriver` struct that wraps `GitRepo`, `Index`, and `SqlEngine` behind a `Mutex`. It provides a UniFFI-friendly API with FFI-safe error types (`ZdbError`) and value types (`SearchResult`, etc.) that mirror the core types but use only UniFFI-compatible primitives. Swift and Kotlin bindings are generated from the proc-macro annotations via the isolated `zdb-uniffi-bindgen` crate.

### Bundled types

The `bundled_types` module (see `bundled_types.rs`) ships predefined typedef templates as embedded Markdown strings. Available templates include `project` (with columns for completed, deliverable, parent, ticket, and body template sections for Description/Log/Plan/Solution), `contact` (aliases, contact-type, email, with folder storage), and `literature-note`. These are installed via `zdb type install <name>`, which writes the typedef zettel into `zettelkasten/_typedef/`.

### Dynamic GraphQL schema

The server reads all `_typedef` zettels at startup via the actor, then builds a dynamic async-graphql schema (see `schema.rs:build_schema()`). Each typedef generates a GraphQL object type with fields matching the column definitions, plus standard zettel fields (id, title, body, tags, links, etc.). The schema includes queries for listing, filtering, searching, and counting typed zettels, plus mutations for CRUD and SQL execution. Subscriptions use the event bus for real-time push.

When a typedef mutation occurs, the `SchemaReloader` rebuilds and atomically swaps in the new schema via `ArcSwap`, so clients see updated types without a server restart.

### PgWire protocol

The server exposes a Postgres wire protocol endpoint (see `pgwire.rs`) that allows standard SQL clients (psql, DBeaver, etc.) to connect and run queries against the SQLite index. SELECT statements are routed to the read pool; DDL/DML goes through the actor. MD5 password authentication uses the same bearer token as the HTTP API.

### REST API

A JSON REST API (see `rest.rs`) provides conventional CRUD endpoints alongside the GraphQL interface. Routes follow the pattern `/rest/zettels`, `/rest/zettels/:id`, with query parameters for filtering and pagination.

### WebSocket subscriptions

The WebSocket endpoint at `/ws` (see `ws.rs`) supports GraphQL subscriptions over the graphql-ws protocol. Authentication can occur either via the HTTP `Authorization` header during upgrade or via the `connection_init` payload for browser clients. Keepalive pings maintain connection health.

### Attachments

The attachment system (see `attachments.rs`) manages binary files associated with zettels. Files are stored in `reference/{zettel_id}/` and tracked in the zettel's frontmatter `attachments` array. Operations include `attach()` (validates the filename, detects MIME type, writes the blob, updates frontmatter, and commits), `detach()` (removes the blob and frontmatter entry), and `list()` (reads from frontmatter). The server provides a direct file-serving endpoint at `/attachments/{zettel_id}/{filename}` with path-traversal protection.

### Bundles

The bundle system (see `bundle.rs`) supports air-gapped sync between devices that cannot reach each other over a network. Bundles are tar archives containing a Git bundle (delta or full), node registration files, a manifest, and a SHA-256 checksum. Export creates a bundle targeting a specific node (delta based on that node's known heads) or a full bundle for backup. Import applies the bundle to the local repository and updates sync state.

### Maintenance

The maintenance module (see `maintenance.rs`) wraps `git maintenance run` for repository housekeeping (commit-graph, loose-objects, incremental-repack, pack-refs). If the system `git` binary lacks the `maintenance` subcommand, it falls back to `git gc --auto`. Auto-triggering can be configured in `.zetteldb.toml` with a write-count threshold. The server optionally runs maintenance on a periodic timer.

### Compaction

The compaction module (see `compaction.rs`) cleans up CRDT temporary files in `.crdt/temp/` and runs Git garbage collection. It computes a shared-head across all active nodes' known heads to determine which CRDT documents are safe to prune. A pre-compaction backup bundle can be created automatically. The `compact()` function reports bytes reclaimed, files removed, and CRDT documents compacted.


## 7. Testing Strategy

ZettelDB uses a layered testing approach with six distinct tiers, each covering different aspects of the system.

### Unit tests

In-module `#[cfg(test)] mod tests` blocks test individual functions in isolation. These run with `cargo test` (the fast local tier) and cover parsing edge cases, CRDT merge logic, SQL translation, type inference, HLC arithmetic, and error mapping. The `MockSource` in `traits::mock` provides an in-memory zettel store for tests that need a `ZettelSource` without touching Git.

### Integration tests

Tests in `zdb-core/tests/` exercise multi-module interactions: sync between two repos, property-based tests for parse/serialize round-trips, and CRDT resolution across different conflict scenarios. Run with `cargo test -p zdb-core`.

### Property tests

Proptest-based tests in `zdb-core/tests/property_tests.rs` generate random zettel content and verify invariants like parse/serialize round-trip fidelity and CRDT merge commutativity. The default case count is suitable for CI; a thorough run (`PROPTEST_CASES=5000`) takes around 20 minutes.

### End-to-end tests

The `tests/e2e/` directory contains assert_cmd-based tests that exercise the `zdb` binary as a black box. These tests create temporary repositories, run CLI commands, and assert on stdout/stderr output and exit codes. Server tests use reqwest to hit the HTTP endpoints. The binary must be built first: `cargo build -p zdb-cli && cargo test -p zdb-e2e`.

### Smoke tests

`tests/smoke.sh` (bash, Linux/macOS) and `tests/smoke.ps1` (PowerShell, Windows) provide quick end-to-end validation of the CLI and server. Each file follows a numbered-section pattern with a `pass` helper for status reporting. A `SMOKE_PROFILE=quick` environment variable runs a minimal subset.

### Benchmarks

Criterion benchmarks in `zdb-core/benches/` measure CRUD operations and search performance at 1K zettels. `crud.rs` benchmarks create, read, update, and delete cycles. `search.rs` benchmarks FTS5 search, SQL SELECT, and full reindex. Run with `cargo bench`; compile-only with `cargo bench --no-run`.

### Full suite

`cargo test --workspace` (aliased as `cargo test-full`) runs all of the above: unit tests, integration tests, and e2e tests across all crates. This is the definitive validation before merging any change.

## 8. Consistency Auto-Fix

The `consistency` module (`consistency.rs`) provides a detect-then-apply pipeline for normalizing zettels. The `zdb fix` command scans all zettels and corrects common data quality issues in a single atomic commit.

### Detection

`detect_fixes(parsed, schema)` inspects a `ParsedZettel` and returns a `Vec<Fix>` with severity-ordered issues:

- **Error**: cross-zone duplicate fields (same key in frontmatter and body inline fields)
- **Warning**: missing type default, missing title (derived from H1 or filename)
- **Info**: duplicate tags, unsorted tags, `#`-prefixed tags, non-kebab-case keys, untrimmed/uncapitalized title, H1-title mismatch

### Application

`apply_fixes(parsed, fixes)` modifies the `ParsedZettel` in-place and re-serializes via `parser::serialize()`. Tag fixes run in order: strip hash, dedup, sort. Key normalization uses `to_kebab_case()` which handles CamelCase, snake_case, and acronyms (e.g. `XMLParser` -> `xml-parser`).

### Orchestration

`fix_all(repo, index, dry_run)` iterates all zettels, detects fixes per typedef schema, applies them, and commits atomically. Dry-run mode collects the report without modifying files.

### Migration

`migrate_all(repo, dry_run)` runs versioned field-level migrations: `zkn-id` -> `id`, `tag` (singular) -> `tags`, type normalization (`loop` -> `project`, `zettel`/`wiki-article` -> `note`). Version tracked in `.zdb/migration-version`. Invoked via `zdb fix --migrate`.

### CLI

```
zdb fix [--dry-run] [-v/--verbose] [--migrate]
```

## 9. Discovery

The discovery system surfaces latent connections, maintenance issues, and knowledge gaps across the zettel graph. Four queries are available via CLI and GraphQL.

### Unlinked mentions

Finds zettels whose body text mentions another zettel's title without linking to it. Uses FTS5 phrase matching against the target's title, then excludes zettels that already link to the target (by path, ID, or alias). Self-references are also excluded.

```bash
zdb discover mentions <id>
zdb discover mentions --all
```

```graphql
query { unlinkedMentions(id: "20260301120000") { sourceId sourceTitle snippet } }
```

### Link suggestions

Suggests related zettels based on a hybrid scoring algorithm. Candidates with shared tags are scored by Jaccard similarity (weighted 0.6), then FTS5 BM25 content similarity against the source title is added (weighted 0.4). Already-linked zettels are excluded. When the source has no tags, the system falls back to content-only scoring.

```bash
zdb discover similar <id> [--limit N]
```

```graphql
query { suggestions(id: "20260301120000", limit: 5) { id title score sharedTags } }
```

### Staleness tracking

Identifies zettels that haven't been updated within their type's configured threshold. Types opt in by setting `stale_after_days` in the typedef frontmatter:

```yaml
stale_after_days: 30
```

The last-updated date uses a priority chain: git revision date > frontmatter `date` field > indexer `updated_at`. Results are sorted by staleness (most stale first).

```bash
zdb discover stale [--type <type>]
```

```graphql
query { staleZettels(type: "project") { id title zettelType lastUpdated dateSource daysStale thresholdDays } }
```

### Orphan detection

Finds zettels with zero incoming links (no other zettel links to them). Typedef zettels are excluded since they are structural, not content nodes. Results include the outgoing link count to help assess whether an orphan is isolated or simply unreferenced.

```bash
zdb discover orphans [--type <type>]
```

```graphql
query { orphanZettels(type: "note") { id title zettelType outgoingLinks } }
```


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

Nested `Map` and `List` values in frontmatter `extra` fields are flattened into the `_zdb_fields` table with dot/bracket notation keys:

```yaml
author:
  name: Alice
  email: alice@example.com
scores:
  - 10
  - 20
```

Produces `_zdb_fields` rows with keys: `author.name`, `author.email`, `scores[0]`, `scores[1]`.

### SQL and GraphQL

Column names containing `.` or `[` trigger path navigation in both:
- SQL engine `extract_column_value` (materialized type views)
- GraphQL `extract_typed_field` (dynamic schema resolvers)

For deeper detail on any module, see the corresponding document in `docs/src/technical/`:

- `technical/parser.md` -- three-zone Markdown parsing
- `technical/git-ops.md` -- Git storage layer
- `technical/crdt-resolver.md` -- conflict resolution strategy
- `technical/indexer.md` -- SQLite index, FTS5, type inference
- `technical/sql-engine.md` -- SQL translation layer
- `technical/sync.md` -- multi-device sync protocol
- `technical/server.md` -- server architecture and API
- `technical/ffi.md` -- UniFFI bindings
- `technical/data-model.md` -- zettel format and frontmatter schema
- `technical/errors.md` -- error handling patterns
