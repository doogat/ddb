# Doogat DB Code Walkthrough

Doogat DB is a hybrid Git-CRDT decentralized Doogat database written in Rust. Git is the source of truth for all data; a SQLite index with FTS5 provides fast reads and full-text search. When concurrent edits on different devices produce Git merge conflicts, an Automerge CRDT layer resolves them at the zone level. The system ships as a CLI, a multi-protocol server (GraphQL, REST, PgWire, WebSocket), and a UniFFI facade for native Swift/Kotlin apps.


## 1. Workspace Layout

The Cargo workspace lives at the repository root. Five members are declared in the root `Cargo.toml`; three of them are default members so that a bare `cargo build` compiles the fast local loop.

### ddb-core

All domain logic lives here: parsing, Git storage, CRDT conflict resolution, SQLite indexing, SQL translation, sync orchestration, compaction, attachments, and the UniFFI FFI facade. Every other crate depends on it. The crate root at `lib.rs` re-exports every public module and calls `uniffi::setup_scaffolding!()` to wire up FFI scaffolding. An optional `nosql` feature gate enables an experimental redb-backed key-value index. The `service` module is a directory module providing a unified `DoogatService` orchestration layer that composes a GitBackend with an injected `IndexPort` and `NoSqlMirrorPort` into a single entry point — CLI, FFI, and server all delegate to it instead of independently composing core modules. The service directory splits concerns across submodules: `mod.rs` (struct, runtime builders `open`/`init`, the `from_parts` injection seam, state management), `create.rs`/`read.rs`/`update.rs`/`delete.rs` (single-doogat CRUD with port-based NoSQL dual-write), `batch.rs` (batch create/create_many for multi-row typed writes), `validation.rs` and `write_helpers.rs` (shared input validation and write/commit helpers), `search.rs` (search, filtered queries, tag/aggregate queries), `sql.rs` (SQL pass-through, transactions), `ops.rs` (compact, maintenance, bundle export), `discovery.rs` (unlinked mentions, sequences, backlinks, bundled types), `utility.rs` (schema queries, NoSQL reads, health check), and `concrete_index.rs` (methods still requiring the concrete `Index`: sync, import_bundle, rename, fix_all, zone_migrate, attach/detach).

### ddb-cli

A clap-derived binary (`ddb`). `main.rs` defines CLI structs and dispatches to `commands/` submodules (`crud.rs`, `query.rs`, `sync.rs`, `maintenance.rs`, `discover.rs`). Each submodule opens a `DoogatService` and delegates to it. Subcommands cover the full lifecycle: `init`, `create`, `read`, `update`, `delete`, `search`, `query`, `sync`, `reindex`, `compact`, `rename`, `serve`, `type`, `node`, `bundle`, `attach`, `detach`, `attachments`, `get`, `scan`, `backlinks`, `maintenance`, `discover`, `sequence`, and `update-bin`. An embedded `updater` module handles self-update from GitHub releases.

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

All domain types are defined in the `types/` directory module (`value.rs`, `doogat.rs`, `schema.rs`). The data model is intentionally flat: every doogat is a Markdown file with structured frontmatter, and the system derives all relational structure from that content.

### DoogatId

A 14-digit timestamp string in the format `YYYYMMDDHHmmss`, for example `"20260226120000"`. It is a newtype wrapper around String (see `types/doogat.rs:DoogatId`). A custom `Deserialize` implementation accepts both YAML integer and string representations for backward compatibility with older doogats whose IDs were serialized as bare numbers.

### Three-zone Markdown

Every doogat file is divided into three zones separated by YAML front-matter fences (`---`):

1. **Frontmatter** -- YAML key-value pairs: id, title, date, type, tags, and arbitrary extra fields captured in a `BTreeMap<String, Value>`.
2. **Body** -- Free-form Markdown content below the closing `---` of the frontmatter.
3. **References** -- An optional third zone below a second `---` fence, used for structured references, parent links, and other relational metadata.

The parser splits these zones via `parser::split_zones()`, which returns a `Doogat` struct holding the three raw strings.

### ParsedDoogat

The fully parsed representation after extracting metadata from all three zones (see `types/doogat.rs:ParsedDoogat`). It holds:

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

### Service freshness contract

Most public `DoogatService` methods that perform validation against the SQLite index call `ensure_fresh()` on entry. `ensure_fresh()` is a thin guard around `Index::rebuild_if_stale(&self.repo)`: when the stored HEAD OID in `_ddb_meta` differs from the current Git HEAD, it triggers `incremental_reindex()` for the changed paths only. The actor / server path opts out via `set_skip_stale_check(true)` because it keeps the index hot in-process between requests (see `service/mod.rs`). The "Known gaps" subsection below enumerates the public methods that perform index validation but do not call `ensure_fresh()` today.

Methods bound by this contract (call sites in `service/create.rs`, `service/read.rs`, `service/update.rs`, `service/delete.rs`, `service/batch.rs`, `service/search.rs`, `service/utility.rs`, `service/discovery.rs`):

- **Reads** — `read_doogat`, `get_doogat_parsed`, `get_doogats_batch`.
- **Typed writes** — `create_doogat_with_extra`, `update_doogat_parsed`, `delete_doogat`.
- **Batch typed writes** — `batch_create`, `batch_update`.
- **Search / list / aggregate** — `search`, `search_paginated`, `search_paginated_filtered`, `list_doogats_filtered`, `count_doogats_filtered`, `list_tags`, `query_tags`, `aggregate_query`, `query_raw_with_params`, `query_raw_with_columns`, `typed_filtered_list`.
- **Schema introspection** — `infer_schema`.
- **Discovery / sequence** — `unlinked_mentions`, `suggest_links`, `stale_doogats`, `orphan_doogats`, `recent_doogats`, `link_density`, `sequence_tree`, `sequence_breadcrumb`, `broken_sequences`, `sequence_info`, `sequence_children`, `backlink_ids`, `all_doogat_ids`.

Adding a new public service method that performs index validation requires adding the same call. The CLI `commands/crud.rs` defence-in-depth pattern (`svc.rebuild_if_stale()?` after `DoogatService::open`) is independent of this — it provides redundancy against a future refactor that drops the service-layer guard.

**Methods that intentionally bypass the contract:**

- `create_doogat_raw` — raw-Markdown create that derives its file path from the parsed `id` + type metadata rather than querying the index. Commits, then runs `index_doogat` and (for typed payloads) `materialize_single` to catch up the index after the write.
- `read_doogat_raw` — takes a git-relative `path` directly and does not query the index for path resolution.

**Known gaps:**

- `rename_doogat` (`utility.rs`) calls `self.index.resolve_path(id)` without `ensure_fresh()`. A cross-process rename of a recently-committed doogat could fail with a stale "doogat not found" error. PRD 00136 documented this gap rather than fixing it; a follow-up will land alongside the FFI-create parity work.
- `update_doogat_raw` (`update.rs`) calls `self.index.resolve_path(id)` without `ensure_fresh()` — same staleness shape as `rename_doogat`. A cross-process raw update of a recently-committed doogat can fail with a stale "doogat not found" error.
- `execute_sql` / `execute_batch` (`sql.rs`) perform FK validation and schema lookup against the index but do not call `ensure_fresh()`. Today's mitigations: the CLI `query` handler calls `svc.rebuild_if_stale()?` on entry (see `commands/crud.rs`), and the actor / GraphQL path keeps the index hot via `set_skip_stale_check(true)`. A direct-FFI consumer that calls `execute_sql` outside both surfaces is responsible for its own freshness guarantees.
- `list_type_schemas` (`utility.rs`) reads `WHERE type = '_typedef'` from the index without `ensure_fresh()`. Internal callers (`create_doogat_with_extra`, `update_doogat_parsed`, `batch_create`, `batch_update`, etc.) already `ensure_fresh()` before invoking it, so the typed-write paths are safe. The actor / GraphQL `read_pool::get_type_schemas` opts out via `set_skip_stale_check(true)` by design. A direct-FFI consumer calling `DdbDriver::list_type_schemas` across process boundaries can see typedef changes from a sibling process delayed until the next freshness-bound call. Same FFI-consumer-responsibility shape as `execute_sql`.

Historical note: issue #16 (PRD 00136) surfaced when `create_doogat_with_extra` was the one typed-write entry point missing the `ensure_fresh()` call, leading to FK-validation rejections across consecutive `ddb create` invocations in the same shell. The user-visible bug was actually closed first by PRD 00134's `materialize_single` wiring (which keeps the on-disk type table in sync after each create); PRD 00136 codified the freshness call so the symmetry held across the typed-write surface.

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

1. `indexer::Index::ensure_fresh()` compares the stored HEAD OID in `_ddb_meta` against the actual Git HEAD. If they differ, a targeted incremental reindex runs for changed paths only (see `indexer/rebuild.rs:incremental_reindex()`).
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
4. For add-add collisions, `crdt_resolver::lww_pick` chooses the winner: the higher HLC wins, falling back to the higher content key when either HLC is missing or the two are equal. That fallback is role-independent, so both devices pick the same winner regardless of merge direction. The loser is carried into the same merge as a `CollisionLoser` — it is not stashed for a later commit. Delete-vs-edit conflicts resurrect the surviving edit. Normal conflicts go through `crdt_resolver::resolve_conflicts()` (see Module Connections below).
5. All resolved files are committed in a single merge commit.
6. Collision losers are folded into that same commit, never a second one (`git_ops::merge::fold_losers_into_index`). Each loser's new ID is derived by `id_minting::derive_content_id` from `(old_id, losing_blob_oid)` — content-addressed rather than wall-clock-minted, so two nodes resolving the same collision independently agree on it with no coordination — and is checked against every ID already present anywhere under `ddb/` in the merge tree. The loser's frontmatter `id` is rewritten, and inbound wikilinks naming the old ID are rewritten in the same index, skipping any reference the winning side's own tree already carries so the winner's backlinks are left intact.
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

### Declarative schema apply (PRD 00161)

The `schema_diff` module (`ddb-core/src/schema_diff/`) is a new module boundary that sits above the SQL engine. It turns a desired-schema YAML document into an ordered DDL plan and applies it.

1. `schema_diff::desired::SchemaDoc::from_yaml()` parses the desired-schema document into one `TableSchema` per declared type, reusing `schema_from_parsed` so the desired vocabulary matches stored typedefs.
2. `service::DoogatService::describe_type()` reads each type's live `TableSchema` (or `None` when absent).
3. `schema_diff::diff()` is a pure function over desired and live schemas; it returns a `SchemaPlan` of ordered `PlanOp`s plus a list of unsupported changes.
4. On apply, the service wraps the plan in one transaction and runs each op via `execute_sql(op.render_sql())`. `render_sql` emits DDL the existing `sql_engine` handlers already accept, so apply reuses the imperative DDL path rather than duplicating it. The whole plan commits as a single Git commit on success or rolls back on any failure.
5. Dry-run returns the `SchemaApplyReport` without mutating. The differ stays pure (no I/O), which keeps diff logic unit-testable in isolation.

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

The SQL engine translates relational operations into doogat file operations. DDL (CREATE TABLE, ALTER TABLE, DROP TABLE) creates, modifies, or deletes `_typedef` doogats in `ddb/_typedef/`. DML (INSERT, UPDATE, DELETE) becomes `git_ops::GitRepo::commit_file()` or `commit_batch()` calls. SELECT goes directly to the SQLite index. The engine holds references to `&dyn SqlBackend` (for index queries, materialization, and raw SQLite access) and `&dyn DoogatStore` (for git operations). `SqlBackend` extends `DoogatIndex` with SQLite connection access and materialization helpers, decoupling `sql_engine` from the concrete `Index` type.

### Server actor bridge

The server uses an actor pattern to bridge async axum handlers with the synchronous core library (see `actor/mod.rs:ActorHandle`). An `mpsc` channel carries `ActorCommand` variants to a background thread. Each command includes a `oneshot` sender for the response. The actor thread owns a `DoogatService` instance, delegating all operations to it via `actor/handlers.rs:handle_command()`. This ensures consistent behavior (NoSQL dual-writes, index freshness) across CLI, FFI, and server entry points.

For read-heavy workloads, a `ReadPool` (see `read_pool.rs`) provides concurrent read-only access. Each read acquires a semaphore permit and runs on `spawn_blocking` with a fresh `DoogatService` instance, bypassing the single-writer actor entirely. The pool size is configurable via server config.

### Indexer rebuild pipeline

Full rebuilds (see `indexer/rebuild.rs:rebuild()`) use a parallel pipeline powered by rayon:

1. `DoogatSource::list_doogats()` collects all doogat paths.
2. `DoogatSource::read_files_batch()` reads file contents (the default implementation is sequential; `GitRepo` can batch).
3. `parser::parse()` runs in parallel across files via rayon's `par_iter()`.
4. Parsed doogats are upserted into SQLite within a transaction.
5. Type definitions are collected and used to materialize typed tables.
6. `RebuildReport` tallies indexed count, materialized tables, inferred types, and any consistency warnings.

Incremental reindex (see `indexer/rebuild.rs:incremental_reindex()`) uses `DoogatSource::diff_paths()` to find changed files since the last known HEAD, then parses and upserts only those files.

### Event bus and subscriptions

The server's `EventBus` (see `events.rs`) is a tokio broadcast channel. After each successful mutation, the actor publishes a `DoogatEvent` with the kind (created/updated/deleted), doogat ID, type, and timestamp. GraphQL subscriptions and WebSocket connections subscribe to this channel for real-time updates.

### Relation filter compiler (PRD 00159)

PRD 00159 introduced `ddb-server/src/relation_filter.rs`, which houses the forward and reverse relation `EXISTS`-over-junction compilers that back the typed `where:` filter's `{T}RelationFilter` and `{T}MembershipFilter` inputs. The `where:`-clause compiler now threads a `WhereCtx` recursion context (depth counter, schema reference) through the single object-walker `build_conditions_into`, so nested relation sub-filters share a consistent depth bound and schema view without re-entering schema lookup on every level.

### Hot schema reload

When a typedef doogat is created, updated, or deleted, the actor triggers a schema reload via `SchemaReloader` (see `reload.rs`). The reloader fetches current type schemas from the actor, rebuilds the dynamic GraphQL schema, and atomically swaps it into the `ArcSwap<Schema>` shared with all request handlers. This allows the GraphQL API to reflect typedef changes without a server restart.


## 5. Key Types and Traits

### Foundation types

- **DoogatId** -- 14-digit timestamp string newtype. See `types/doogat.rs:DoogatId`.
- **CommitHash** -- Git commit OID string newtype. Access the inner string via `.0`. See `types/doogat.rs:CommitHash`.
- **Value** -- Domain-level value enum (String, Number, Bool, List, Map), decoupled from serde_yaml. See `types/value.rs:Value`.
- **Zone** -- Enum identifying which part of the doogat data came from: Frontmatter, Body, or Reference. See `types/doogat.rs:Zone`.

### Doogat types

- **Doogat** -- Raw three-zone split before metadata extraction: `raw_frontmatter`, `body`, `reference_section`. See `types/doogat.rs:Doogat`.
- **DoogatMeta** -- Core metadata from YAML frontmatter with an `extra` BTreeMap for arbitrary fields. See `types/doogat.rs:DoogatMeta`.
- **ParsedDoogat** -- Full parsed representation with all extracted metadata, links, inline fields, sections, checkboxes, and body tags. See `types/doogat.rs:ParsedDoogat`.
- **InlineField** -- A Dataview-style `key:: value` pair with its source zone. See `types/doogat.rs:InlineField`.
- **Link** -- An extracted reference with target, display text, section anchor, kind, and zone. See `types/doogat.rs:Link`.
- **LinkKind** -- Discriminant for link syntax: WikiLink, MarkdownLink, Embed, BareUrl. See `types/doogat.rs:LinkKind`.
- **Section** -- A heading/content pair parsed from the body. See `types/doogat.rs:Section`.
- **CheckboxItem** -- A task item with state, content, dates, line number, and indent level. See `types/doogat.rs:CheckboxItem`.

### Sync and merge types

- **NodeConfig** -- Per-device registration stored in `.nodes/`. Tracks UUID, name, known heads, last sync time, HLC, and lifecycle status (Active/Stale/Retired). See `types/doogat.rs:NodeConfig`.
- **MergeResult** -- Outcome of `git_ops::merge_remote()`: AlreadyUpToDate, FastForward, Clean, or Conflicts. See `types/doogat.rs:MergeResult`.
- **ConflictFile** -- All three versions of a conflicted file plus optional HLC timestamps. See `types/doogat.rs:ConflictFile`.
- **ResolvedFile** -- Merged content after CRDT resolution, with optional serialized CRDT bytes. See `types/doogat.rs:ResolvedFile`.
- **SyncReport** -- Summary of a sync operation: direction, commits transferred, conflicts resolved, resurrected files, and collisions reassigned. See `types/doogat.rs:SyncReport`.

### Schema types

- **TableSchema** -- Schema for a materialized SQLite table: name, columns, CRDT strategy, template sections, and folder flag. See `types/schema.rs:TableSchema`.
- **ColumnDef** -- A column definition with name, data type, optional FK reference, zone mapping, required flag, search boost, allowed values, and default. See `types/schema.rs:ColumnDef`.

### Report types

- **RebuildReport** -- Rebuild statistics: indexed count, tables materialized, types inferred, and consistency warnings. See `types/schema.rs:RebuildReport`.
- **CompactionReport** -- Compaction statistics: files removed, CRDT docs compacted, gc success, size metrics, and backup path. See `types/doogat.rs:CompactionReport`.
- **MaintenanceReport** -- Git maintenance results: tasks run, success flag, duration, and fallback indicator. See `types/doogat.rs:MaintenanceReport`.
- **RenameReport** -- Rename results: updated file list and unresolvable references. See `types/doogat.rs:RenameReport`.
- **SearchResult** and **PaginatedSearchResult** -- Search hits with id, title, path, snippet, rank, and pagination metadata. See `types/doogat.rs:SearchResult`.

### Error handling

`DoogatError` is a single thiserror-derived enum covering all failure modes: Git, Yaml, Sql, Automerge, Io, Toml, Parse, NotFound, Validation, InvalidPath, SqlEngine, Conflict, Sync, Index, BadRequest, and VersionMismatch. An optional `Redb` variant exists behind the `nosql` feature gate. See `error.rs:DoogatError`.

The crate defines a `Result<T>` alias as `std::result::Result<T, DoogatError>`. Every public function in the library returns this type. No panics in library code; all errors propagate via `?`.

### Core traits

Eleven traits define the module boundaries (see `traits.rs`):

- **DoogatSource** -- Read-only access to doogat storage: `list_doogats()`, `read_file()`, `head_oid()`, `diff_paths()`, and `read_files_batch()`. Implemented by `GitRepo`. The batch read has a default sequential implementation that concrete types can override.
- **DoogatStore** -- Read-write access extending DoogatSource: `commit_file()`, `commit_files()`, `delete_file()`, `delete_files()`, and `commit_batch()`. Implemented by `GitRepo`.
- **GitRemote** -- Remote operations: `add_remote()`, `fetch()`, `push()`.
- **GitMerge** -- Merge operations: `merge_remote()`, `commit_merge()`.
- **GitHistory** -- Commit introspection, tree walking, history queries: `merge_base()`, `commit_parent_count()`, `commit_parent_oid()`, `read_file_at()`, `walk_tree_files()`, `find_hlc_for_path()`, `revision_date()`.
- **GitBinary** -- Binary file operations: `commit_binary_file()`, `commit_binary_and_text()`, `read_blob()`.
- **GitRename** -- File rename: `rename_file()`.
- **GitDesktopHooks** -- Desktop-only hooks with default no-ops: `set_skip_commit_graph()`, `write_commit_graph()`, `increment_session_commits()`, `reset_session_commits()`.
- **GitBackend** -- Supertrait composing DoogatSource + DoogatStore + GitRemote + GitMerge + GitHistory + GitBinary + GitRename + GitDesktopHooks. Adds `repo_path()` and `load_config()`. Implemented by `GitRepo`. Enables swapping libgit2 for gitoxide per-feature. All OIDs are passed as `&str` hex strings (no `git2` types in any trait).
- **DoogatIndex** -- Query and mutation operations on the search index: `index_doogat()`, `remove_doogat()`, `search()`, `search_paginated()`, `resolve_path()`, `query_raw()`, `find_typedef_path()`, and `execute_sql()`. Implemented by `Index`.
- **ConflictResolver** -- CRDT-based conflict resolution: `resolve_conflicts()` takes a list of `ConflictFile` structs and an optional strategy string, returning `ResolvedFile` results. The free function `crdt_resolver::resolve_conflicts()` implements this logic.

`DoogatService<G: GitBackend = GitRepo, I: IndexPort = Index>` is generic over both the git backend and the index port. The default type parameters mean existing code using bare `DoogatService` resolves to `DoogatService<GitRepo, Index>` without changes. The index is held as an injectable `I: IndexPort` and the NoSQL mirror as a `Box<dyn NoSqlMirrorPort + Send + Sync>`, so service logic depends on abstractions rather than the concrete SQLite `Index`/Redb adapters. `DoogatService::from_parts(repo, index, nosql, repo_path)` is the dependency-injection seam; `open`/`init` are the default runtime builders that construct the concrete adapters (and `.ddb` directory) at the edge and wrap `from_parts`. A `MockSource` in `traits::mock` provides an in-memory `DoogatSource`, and `service/mock_index_tests.rs` drives a service test through a mock `IndexPort` plus `NoopMirror` with no real SQLite index. A small `service/concrete_index.rs` block holds the few methods (`sync`, `import_bundle`, `rename_doogat`, `fix_all`, `zone_migrate_all`, `attach_file`, `detach_file`) that still pass `&self.index` to collaborators typed against the concrete `&Index`; these live on `DoogatService<G, Index>` to avoid generifying consistency/sync/bundle/attachments.

### Hybrid Logical Clock

The `Hlc` struct (see `hlc.rs:Hlc`) combines a wall-clock millisecond timestamp, a logical counter, and a truncated node ID (first 8 characters of the UUID). Two operations maintain causality:

- `Hlc::now()` -- tick the clock for a local event, advancing the wall time or bumping the counter.
- `Hlc::recv()` -- merge on receive, taking the max of local, remote, and wall time, then bumping the counter on ties.

HLC values are totally ordered and used by the last-writer-wins CRDT strategy to determine which version survives a conflict.


## 6. Extension Points

### UniFFI facade

The FFI module (see the `ffi/` directory: `driver.rs` for the object, `records.rs` for the FFI-safe types) exposes a `DoogatDriver` struct that wraps `GitRepo` and `Index` behind a `Mutex`, creating `SqlEngine` instances on demand. It provides a UniFFI-friendly API with FFI-safe error types (`DdbError`) and value types (`SearchResult`, etc.) that mirror the core types but use only UniFFI-compatible primitives. Swift and Kotlin bindings are generated from the proc-macro annotations via the isolated `ddb-uniffi-bindgen` crate.

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

### Core API (types/)

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

When `CREATE TABLE` defines columns, each column's SQL data type determines its default zone placement. This inference runs in `sql_engine/ddl.rs:extract_columns()` during typedef creation.

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

### Auto-creation (sql_engine/ddl.rs)

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


## 16. Singleton Typedefs

PRD 00139 adds a `SINGLETON` typedef primitive: a typedef constrained to hold at most one row. Apps modeling app config, schema version, or other state-of-one rows use it to skip the existence-check / seed / update-without-id workaround. Enforcement is defence-in-depth at three layers; a sync-time CRDT sweep resolves the offline-write race.

### Schema flag

`types/schema.rs::TableSchema` carries `pub singleton: bool` alongside the other table-level booleans (`folder`, `unique_together`). Serde skips serializing when `false`, so legacy typedef YAML is byte-identical to pre-PRD output.

### DDL parsing (sql_engine/helpers.rs)

`SINGLETON` and `SINGLETON DEFAULT VALUES` are not ANSI SQL. A regex pre-scan in `helpers.rs` detects the markers on `CREATE TABLE ... SINGLETON [DEFAULT VALUES]`, strips them, and hands the residue to `sqlparser`. `handle_create_table` reads the captured flags into the `TableSchema`. `ALTER TABLE x SET SINGLETON` / `ALTER TABLE x DROP SINGLETON` dispatch to `handle_set_singleton` / `handle_drop_singleton`, modeled on the `handle_rename_table` (PRD 00132) and `handle_alter_column_type` (PRD 00128) patterns.

### Three-layer enforcement

1. **Typed-write validation** -- `service/validation.rs::check_singleton_constraint` queries the materialized table for `COUNT(*) >= 1` before any INSERT through the service layer. On hit, returns `DoogatError::singleton_violation(table, existing_id)`.
2. **SQL DML pre-check** -- `sql_engine/dml.rs` INSERT Pass 1 runs the same check before any commit lands, and rejects two-rows-in-one-batch (`rows.len() > 1` against an empty table) with `existing_id = "<intra-batch>"`. The service-layer batch path in `service/batch.rs` uses a `seen_singleton` HashMap for the same purpose across a multi-row typed write.
3. **Materializer UNIQUE index** -- `indexer/materialize.rs::create_singleton_lock_index` issues `CREATE UNIQUE INDEX <table>_singleton_lock ON <table> ((1))`. The expression-index trick rejects any second materialized row even when the upstream service path is bypassed (direct git write, manual reindex).

The three layers are integration-tested together in `ddb-core/tests/singleton_layers.rs` and must produce byte-identical structured errors.

### Cross-process write safety (PRD 00140)

The three layers keep the database correct under contention, but Layer 1's check-then-write is a TOCTOU window across processes: a second process holding the same repo (a CLI invocation alongside a running server, two servers, or two `ddb create` invocations) can land its own row between the first process's pre-check and its materialize. PRD 00140 closes that window by wrapping each service write path that targets a registered SINGLETON typedef -- `create_doogat_raw`, `create_doogat_with_extra`, `batch_create`, `update_doogat_raw` -- so its constraint-check, git write, and index update run inside a single `BEGIN IMMEDIATE` SQLite transaction. `BEGIN IMMEDIATE` takes the database write lock up front, serializing concurrent writers at the file-lock level: the second process blocks on the lock, and when it acquires the lock its Layer 1 pre-check sees the first writer's already-committed row and fires the structured `SINGLETON_VIOLATION` -- never a raw `UNIQUE constraint failed` SQL-error leak.

`upsert<TypeName>` runs its existence check and the create-or-update branch under one `BEGIN IMMEDIATE` window via `DoogatService::upsert_singleton`, so two concurrent upserts on an empty SINGLETON typedef converge on one row: one call returns `created: true`, the loser takes the UPDATE branch and returns `created: false` against the same id.

Trade-off: every SINGLETON write serializes through the SQLite write lock. This is acceptable by definition -- a SINGLETON typedef holds one row, so write throughput is irrelevant. Cross-process behavior is exercised by the e2e tests `singleton_cross_process_create` / `singleton_cross_process_upsert` and `tests/integration.{sh,ps1}` section 55.

### Auto-seed on origin-only

`CREATE TABLE x (...) SINGLETON DEFAULT VALUES` triggers an origin-node-only seed at typedef install time, using each column's `default_value`. Parse-time validation rejects with a clear error listing non-nullable columns without defaults. The seed row and the typedef YAML share one git commit boundary; other nodes inherit the row via normal CRDT sync, not local auto-seed.

### GraphQL surfaces

`schema/queries.rs` and `schema/mutations/singleton.rs` (dispatched from `mutations/mod.rs`) branch on `schema.singleton`:

- Singular query field `<typeName> { id, ...fields }` (no args, returns the row or null).
- `update<TypeName>(input:)` mutation (no id arg; `SINGLETON_NOT_FOUND` when empty).
- `upsert<TypeName>(input:)` mutation returning `{ id, created: Boolean! }`.
- `create<TypeName>(input:)` stays generated and rejects with `SINGLETON_VIOLATION` once a row exists.

Field naming uses `pluralize_preserving_case` for the plural and the bare base name for the singular. Hyphenated typedefs that would collide with another generated field fall back to `<typeName>Singleton`; a double-collision fails schema build rather than masking the conflict.

### CRDT post-sync sweep (consistency/singleton_sweep.rs)

Offline writes on two nodes can each land a row in the same SINGLETON typedef. `singleton_sweep` runs after `finalize_sync`:

1. Group rows by typedef where `schema.singleton == true`.
2. Pick the winner by highest HLC; tie-break deterministically by `(node_id, doogat_id)`.
3. Move losers to `ddb/_conflicts/{losing_id}.md` with frontmatter marker `singleton_conflict_loser: <winning_id>`.
4. Emit a `Fix::SingletonConflictResolved` event into the existing event pipeline; `SyncReport` surfaces the count and detail for the operator.

### Error codes (error.rs)

- `SINGLETON_VIOLATION` -- context carries `table` and `existing_id`.
- `SINGLETON_NOT_FOUND` -- context carries `table`.

`ddb-server/src/error.rs::to_graphql_error` auto-maps these to `extensions.code`; REST `rest_error` maps to HTTP 409 / 404; FFI surfaces them as structured `DdbError::Validation` / `DdbError::SqlEngine` variants.


## 17. Help Guides

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
