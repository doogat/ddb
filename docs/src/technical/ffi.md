# FFI Bindings

> **Experimental**: UniFFI bindings are experimental and may change in future releases. Do not depend on API stability.

**Source**: `ddb-core/src/ffi/` (`mod.rs`, `driver.rs`, `records.rs`, `tests.rs`)

UniFFI-based foreign function interface exposing Doogat DB to Swift and Kotlin via a high-level `DoogatDriver` facade.

## Architecture

```text
Swift/Kotlin app
      │
      ▼
DoogatDriver (ffi/driver.rs) ← UniFFI proc-macro boundary
      │
      ├── GitRepo            ← git_ops (storage)
      ├── Index              ← indexer (search/query)
      ├── SyncManager        ← sync_manager (compact)
      └── parser             ← parse/serialize
```

`DoogatDriver` wraps a single `Mutex<DoogatService>` for thread safety; every method reaches the service through the central `with_service` / `with_service_mut` helpers. All methods take `&self` (shared reference via `Arc` on the foreign side).

## Mobile Integration Model

`DoogatDriver` is the embedded runtime boundary for native apps — not a mobile IPC mechanism. On mobile, the recommended model is one host app embedding a single `DoogatDriver` instance, not multiple separately installed apps communicating over `localhost`. Mobile OS sandboxing and background execution limits make inter-app backends non-portable.

Widgets and extensions (iOS WidgetKit, Android AppWidgetProvider) access the same shared repository via App Group storage (iOS) or app-private storage (Android). Whether extensions get their own read-only `DoogatDriver` instance or consume snapshots exported by the host app depends on the coordination strategy — see the [Building Apps guide](../guide/building-apps.md#mobile-mini-apps) for architecture details.

## Promise Boundaries

> **Stability note**: The **Experimental** label at the top of this page refers to API surface stability - method signatures and record shapes may change before a stable release. It does not mean CRUD operations are unreliable. Behavioral promises are tracked independently.

The embedded interface makes the following promises, consistent with the product's 120% parity posture:

| Capability | Promise | Notes |
|-----------|---------|-------|
| Create / read / update / delete (base CRUD methods or typed SQL via `execute_sql`) | `Guaranteed` | Same Git-backed semantics as CLI and GraphQL |
| FTS5 search (`search`, `search_paginated`, `search_filtered`) | `Guaranteed` | Same `Index` path as the server; `search_paginated` and `search_filtered` route through the app contract's `SearchCommand` |
| SQL (DDL, DML, SELECT via `execute_sql`) | `Guaranteed` | Delegates to the same `SqlEngine` as `ddb serve` |
| Transactions (`begin_transaction`, `commit_transaction`, `rollback_transaction`) | `Guaranteed` | Multi-statement atomicity; not available per-call over GraphQL - use `executeBatch` there |
| Type discovery (`list_type_schemas`) | `Guaranteed` | Returns typedef doogats with column metadata |
| Attachments (attach, detach, list) | `Specialized` | Attachments feature is Experimental per the stability tier table; methods exist and work, but the capability promise is narrower than core CRUD until Attachments moves Stable |
| Maintenance (reindex, compact) | `Guaranteed` | Local in-process operations |
| Bundle export/import | `Specialized` | Available via `export_full_bundle` / `export_delta_bundle` / `import_bundle`; CLI is the canonical bundle workflow (`ddb bundle export/import`) — FFI exposes the same engine for in-process orchestration |
| Remote sync (push/pull/fetch) | `Specialized` | Bundle export/import only; no Git remote push/pull/fetch method on `DoogatDriver`. Ongoing Git remote sync uses CLI `ddb sync` or the GraphQL `sync` mutation |
| Real-time subscriptions / change streaming | `Intentionally absent` | No GraphQL-subscription equivalent; poll or call `reindex()` after writes |
| Cross-process or concurrent drivers | `Intentionally absent` | One `DoogatDriver` per process; do not share a repo between concurrently running drivers |
| Auth / token management | `Intentionally absent` | Host app owns the repo path; no server auth needed or supported |

### Capability gaps vs CLI and GraphQL

| Feature | CLI | GraphQL | FFI (embedded) |
|---------|-----|---------|----------------|
| CRUD baseline | `Guaranteed` | `Guaranteed` | `Guaranteed` |
| Typed SQL (DDL, DML, SELECT) | `Guaranteed` | `Guaranteed` | `Guaranteed` |
| FTS5 search | `Guaranteed` | `Guaranteed` | `Guaranteed` |
| Multi-statement transactions | implicit per-command | `executeBatch` (atomic batch) | `begin_transaction` / `commit_transaction` |
| Real-time push / subscriptions | `Intentionally absent` | `Guaranteed` (WebSocket) | `Intentionally absent` |
| Remote sync (push/pull/fetch) | `Guaranteed` (`ddb sync`) | `Guaranteed` (via `sync` mutation) | `Specialized` (bundles only) |
| API surface stability | Stable | Stable | **Experimental** - signatures may change |

See [Choosing an interface](../guide/building-apps.md#choosing-an-interface) for the top-level recommendation matrix.

## Interface

### Constructors

```rust
DoogatDriver::new(repo_path: String) -> Result<Self, DdbError>
DoogatDriver::create_repo(repo_path: String) -> Result<Self, DdbError>
```

`new` opens an existing Doogat DB repo. `create_repo` creates a new repo (directories, `.gitignore`, initial commit) then opens it. Both set up the SQLite index at `.ddb/index.db`.

### CRUD

| Method | Delegates to |
|--------|-------------|
| `create_structured(title, tags, doogat_type, body, fields, on_conflict)` | `CreateCommand` → `DoogatService::create` |
| `read_doogat(id)` | `index.resolve_path` → `repo.read_file` |
| `update_structured(id, input)` | `UpdateCommand` → `DoogatService::update` |
| `delete_doogat(id, message)` | `index.resolve_path` → `repo.delete_file` → `index.remove_doogat` |

#### Structured create and update (PRD 00177)

```rust
create_structured(
    title: Option<String>,
    tags: Vec<String>,
    doogat_type: Option<String>,
    body: Option<String>,
    fields: Vec<FieldEntryValue>,
    on_conflict: ConflictActionRecord,
) -> Result<CreateResultRecord, DdbError>

update_structured(
    id: String,
    input: UpdateStructuredInput,
) -> Result<UpdateResultRecord, DdbError>

pub struct UpdateStructuredInput {
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub doogat_type: Option<String>,
    pub body: Option<String>,
    pub fields: Vec<FieldEntryValue>,
    pub unset_fields: Vec<String>,
}
```

Both methods build an app-contract command (`CreateCommand` / `UpdateCommand`)
and call `DoogatService::create` / `DoogatService::update`, so FFI runs the same
business policy — typedef routing, validation, id minting, git commit — as CLI,
GraphQL, and REST (see [app-contract.md](app-contract.md#command-dtos)). There
is no `message` argument: the service writes the commit message itself.

`fields` carries typed frontmatter values as a list of
`FieldEntryValue { key, value: FieldValue }` pairs, because UniFFI records carry
no maps. On update, a `None` member leaves the stored value unchanged and
`unset_fields` removes frontmatter keys.

`on_conflict: ConflictActionRecord` (`Error` or `Ignore`) makes
duplicate-identity handling an explicit caller input instead of a hidden
default. `create_structured` passes `UnregisteredTypePolicy::BaseOnly`, so an
unregistered `doogat_type` creates a base doogat and returns an
`UNREGISTERED_TYPE_BASE_ONLY` warning instead of failing — the CLI's lenient
contract, not GraphQL's and REST's strict one.

Both result records carry the doogat's meta plus the warning channel:

- `CreateResultRecord` — `{ id, title: Option<String>, doogat_type: Option<String>, tags: Vec<String>, path, warnings: Vec<RebuildWarningRecord> }`
- `UpdateResultRecord` — same shape

`warnings` is the `AppOutput` warning list, so warnings such as
`UNREGISTERED_TYPE_BASE_ONLY` and `REINDEX_SKIPPED_FILES` now reach
Swift/Kotlin callers instead of being dropped at the boundary.

**Breaking change, no deprecation window.** The raw-markdown exports
`create_doogat(content, message)` and `update_doogat(id, content, message)` are
gone. The maintainer decision recorded in PRD 00177 keeps FFI CRUD
`Guaranteed` and ships the replacement with no deprecation period, because the
bindings are Experimental and the product is pre-1.0. `read_doogat(id)` and
`delete_doogat(id, message)` are unchanged.

| Old call | New call |
|----------|----------|
| `create_doogat(content, message)` → new id | `create_structured(title, tags, doogat_type, body, fields, on_conflict)` → `CreateResultRecord`; split the Markdown you used to serialize into `title` / `tags` / `doogat_type` / `body` / `fields`, drop `message`, and read the new id from `CreateResultRecord.id` |
| `update_doogat(id, content, message)` → `()` | `update_structured(id, UpdateStructuredInput { .. })` → `UpdateResultRecord`; send only the members you want to change instead of a whole replacement document, list removed frontmatter keys in `unset_fields`, and drop `message` |

### Query

| Method | Delegates to |
|--------|-------------|
| `search(query)` | `DoogatService::search` (unfiltered, no pagination), returns hits only |
| `search_paginated(query, limit, offset)` | `PageRequest::decode` → `DoogatService::search_command` (no filters) |
| `search_filtered(query, limit, offset, types, tag)` | `PageRequest::decode` → `DoogatService::search_command` with `types` / `tag` |
| `list_doogats()` | `repo.list_doogats` |
| `execute_sql(sql)` | See [SQL (SqlEngine-backed)](#sql-sqlengine-backed) |

#### Filtered search (PRD 00176)

```rust
search_filtered(
    query: String,
    limit: Option<u32>,
    offset: Option<u32>,
    types: Option<Vec<String>>,
    tag: Option<String>,
) -> Result<PaginatedSearchResult, DdbError>
```

`search_filtered` builds a `SearchCommand` (see
[app-contract.md](app-contract.md#search-searchcommand-pagerequest-appsearchresult)),
so it uses the same filters and the same pagination policy as CLI, GraphQL, and
REST. `types` matches any listed type. `tag` matches one tag. An omitted `limit`
gives 50 and an omitted `offset` gives 0. A `limit` of 0 or above 10,000 fails
with `DdbError::Validation`.

`search_paginated` keeps its signature but now decodes through `PageRequest` too.
**Compatibility:** a `limit` of 0 or above 10,000 used to run without bounds. It
now fails with `DdbError::Validation`. Calls with a `limit` in 1-10,000 behave as
before. The unpaginated `search(query)` did not change.

### Attachments

| Method | Delegates to |
|--------|-------------|
| `attach_file(doogat_id, file_path)` | `fs::read` → `attachments::attach_file` |
| `detach_file(doogat_id, filename)` | `attachments::detach_file` |
| `list_attachments(doogat_id)` | `attachments::list_attachments` |

`attach_file` reads the file from disk, detects MIME type from the filename extension, stores the blob under `reference/{id}/`, updates frontmatter, and returns `AttachmentInfo`. Both repo and index locks are held for the duration.

### SQL (SqlEngine-backed)

| Method | Behavior |
|--------|----------|
| `execute_sql(sql)` | Delegates to `SqlEngine::execute` — same path as `ddb serve`. DDL creates typedef doogats via Git; DML reads/writes Git-backed doogats; SELECT returns rows. |
| `begin_transaction()` | Opens a SAVEPOINT; subsequent `execute_sql` calls buffer writes |
| `commit_transaction()` | Flushes buffered writes/deletes as a single Git commit, releases SAVEPOINT |
| `rollback_transaction()` | Discards buffered writes, rolls back SAVEPOINT |

Returns `SqlResultRecord`:
- **Queries** (SELECT): `columns` and `rows` populated, `affected_rows` = row count
- **Mutations** (UPDATE/DELETE): `affected_rows` populated
- **DDL** (CREATE/DROP TABLE): `message` populated (e.g. "table foo created")
- **INSERT**: `message` contains comma-separated created doogat IDs

### Type Discovery

| Method | Returns |
|--------|---------|
| `list_type_schemas()` | `Vec<TypeSchemaRecord>` — all typedef doogats with columns, CRDT strategy, template sections |

`TypeSchemaRecord` contains:
- `table_name` — the type name
- `columns: Vec<ColumnDefRecord>` — each with `name`, `data_type`, optional `references`, `required` flag
- `crdt_strategy` — optional CRDT merge strategy
- `template_sections` — section names from the typedef template

### Maintenance

| Method | Delegates to |
|--------|-------------|
| `reindex()` | `index.rebuild` |
| `run_maintenance()` | `service::run_maintenance`, returns the run's `success` flag |
| `register_node(name)` | `sync_manager::register_node`, returns UUID |
| `compact()` | `SyncManager::open` → `compaction::compact` |
| `export_full_bundle(output_path)` | `bundle::export_full_bundle`, returns path |
| `export_delta_bundle(target_node_uuid, output_path)` | `bundle::export_bundle`, returns path |
| `import_bundle(bundle_path)` | `bundle::import_bundle` |

## Error Mapping

`DdbError` is the UniFFI-exported error enum. Which variant a core error becomes
is derived from the single error-policy table in
`ddb-core/src/app_contract/error.rs` (PRD 00177): `From<DoogatError>` calls
`error_rendering(AppError::from(&err))` and `ffi/records.rs::category_variant`
maps the resulting `ErrorRendering.ffi_variant` onto a variant. There is no
FFI-local error allowlist any more, so FFI cannot disagree with CLI, GraphQL,
REST, or NoSQL HTTP about an error's category.

| `ErrorRendering.ffi_variant` | AppError category | `DdbError` |
|------------------------------|-------------------|-----------|
| `FfiVariant::NotFound` | `NotFound` | `NotFound { msg }` |
| `FfiVariant::Validation` | `InvalidInput` | `Validation { msg, code, context }` |
| `FfiVariant::Conflict` | `Conflict` | `Conflict { msg, code, context }` |
| `FfiVariant::Internal` | `Internal` | `Io { msg }` |

Two arms bypass the policy on purpose:

| DoogatError | DdbError |
|------------|---------|
| `VersionMismatch { repo, driver }` | `VersionMismatch { msg }` — the schema-version guard, checked before the policy |
| `Redb(msg)` (nosql feature) | `Io { msg }` |

`Conflict { msg, code, context }` is new in PRD 00177. It and `Validation` carry
`code: Option<String>` — the policy's error code, now populated for every error
on these paths, including ones from unstructured sources where it used to be
`None` — and `context: Vec<DdbErrorContextEntry>`, which holds a `Structured`
violation's context and is empty otherwise. `msg` is always the core error's own
`Display` text, so an `Io` error keeps its `io: ` prefix and an internal error
reaches the embedding app unredacted; the policy's message redaction applies to
the network transports, not to this in-process one.

**Breaking change (the second one in PRD 00177).** `Git`, `Yaml`, `Sql`,
`Automerge`, `Parse`, `Config` and `SqlEngine` are still declared in the
generated bindings but no path produces them any more
(`ddb-core/src/ffi/tests/error_parity_tests.rs` pins that). Internal failures —
git, sync, index, automerge, yaml, toml — all arrive as `Io`;
`SINGLETON_NOT_FOUND` arrives as `NotFound` instead of `Validation`; and
`SINGLETON_VIOLATION`, `REFERENCES_VIOLATION`, `CASCADE_CYCLE`,
`UNIQUE_VIOLATION` and `SCHEMA_DESTRUCTIVE_BLOCKED` arrive as `Conflict`
instead of `Validation` or `SqlEngine`. Swift/Kotlin code that branches on the
retired variants must be rewritten against the four live ones, and the bindings
regenerated.

### Service lock access

`DoogatDriver` holds its `DoogatService` behind a `Mutex`. Every FFI method
reaches the service through the central helpers `with_service` and
`with_service_mut`, never `lock().unwrap()`. If an earlier call panicked while
holding the lock the mutex is poisoned; the helpers map `PoisonError` to
`DdbError::Io { msg: "service lock poisoned: ..." }` instead of letting the
panic cross the FFI boundary. Callers therefore always receive a typed error
after a poisoned lock, never a Rust panic.

## FFI Records

- `SearchResult` — `{ id, title, path, snippet, rank, updated_at, tags: Vec<String>, doogat_type: Option<String>, fields: Vec<FieldEntry>, created_at: Option<String> }`, all ten fields of `types::SearchResult`. PRD 00176 widened it from six fields; the change is additive (the first six keep their names and types). `fields` is empty for untyped hits.
- `FieldEntry` — `{ key, value }`, one typed-field pair of a search hit. UniFFI records here carry no maps, so `types::SearchResult.fields` (`BTreeMap<String, String>`) crosses the boundary as a list of pairs in key order.
- `PaginatedSearchResult` — `{ hits: Vec<SearchResult>, total_count: u64, warnings: Vec<RebuildWarningRecord> }`. PRD 00176 added `warnings`; the change is additive, and the list is empty when the search raised none.
- `RebuildReport` — `{ indexed, tables_materialized, types_inferred, warnings: Vec<RebuildWarningRecord> }` (PRD 00169 added `warnings`)
- `AttachmentInfo` — `{ name, mime, size, path }` (mirrors `types::AttachmentInfo`)
- `SqlResultRecord` — `{ columns, rows, affected_rows, message }` (flat conversion from `SqlResult` enum)
- `TypeSchemaRecord` — `{ table_name, columns, crdt_strategy, template_sections }`
- `ColumnDefRecord` — `{ name, data_type, references, required }`
- `DdbErrorContextEntry` — `{ key, value: Option<String>, values }` (structured-error context attached to the `Validation` / `Conflict` variants)

## Compatibility and Deprecation

Promise labels (`Guaranteed`, `Specialized`, `Intentionally absent`, `Deprecated`) are defined in [Compatibility and Deprecation](../guide/building-apps.md#compatibility-and-deprecation) in the building-apps guide. Every deprecated behavior below names a replacement; entries flagged "Status: planned, not yet implemented" reference candidate follow-up PRD slugs that have not shipped yet.

### Deprecated behavior

- **`DoogatDriver.search` offers no per-code error variant**: since PRD 00177 the variant is policy-derived (an internal engine failure arrives as `DdbError::Io`, a rejected input as `DdbError::Validation` carrying the policy `code`), so the old `DdbError::Sql(msg)` shape is gone — but there is still no variant per error code, and a consumer that must branch on a specific internal failure reads `msg`. Replacement: candidate `ffi-typed-errors-v1` follow-up PRD will expose AppError codes as typed FFI enum variants on `DoogatError`, mirroring the AppError envelope PRD 00147 shipped for the network transports. Status: planned, not yet implemented.
- **`DoogatDriver.read_doogat` not-found carries no id payload**: a missing id throws `DdbError::NotFound { msg }` (policy-derived since PRD 00177, not the old `DdbError::Io` "not found" shape), but the id itself is recoverable only from the message text. Replacement: candidate `ffi-typed-errors-v1` follow-up PRD adds `DdbError::NotFound { id }` as a typed variant, mirroring AppError's `NOT_FOUND` code shipped by PRD 00147. Status: planned, not yet implemented.
- **`DoogatDriver.delete_doogat` followed by `read_doogat` returns the same not-found shape**: the post-delete read throws the same payload-free `DdbError::NotFound { msg }`, so consumers cannot distinguish delete-then-read from any other missing id without reading the message. Replacement: same typed `NotFound` variant under candidate `ffi-typed-errors-v1`. Status: planned, not yet implemented.
- **Resolved (PRD 00177): `DoogatDriver.create_doogat` invalid-type error embedded in `DdbError::Sql(msg)`**: `create_doogat` is gone. `create_structured` treats an unregistered `doogat_type` as a base-only create with an `UNREGISTERED_TYPE_BASE_ONLY` warning instead of an error, and the errors it does raise carry the policy's code in the `code` field of `DdbError::Validation` / `DdbError::Conflict`, so nothing has to parse `msg` for `TYPE_NOT_REGISTERED`. A dedicated variant per error code is still deferred to candidate `ffi-typed-errors-v1`.
- **Some FFI return types omit the structured warning channel**: `create_structured` and `update_structured` return `warnings: Vec<RebuildWarningRecord>` (PRD 00177), as `PaginatedSearchResult` (PRD 00176) and `RebuildReport` (PRD 00169) do; the remaining return types still drop AppWarning entries at the UniFFI boundary. Replacement: candidate `ffi-typed-errors-v1` follow-up PRD adds a structured-warnings surface to the rest (e.g. `AppOutput<T>`-equivalent UniFFI record carrying `warnings: Vec<WarningEntry>`), mirroring the AppOutput shape PRD 00147 established for the network transports. Status: planned, not yet implemented.

### Specialized capabilities

Not deprecated, still binding promises:

- **Attachments** — Attachments feature is Experimental per the stability tier table; methods exist and work, but the capability promise is narrower than core CRUD until Attachments moves Stable.
- **Bundle export/import** — CLI is the canonical bundle workflow (`ddb bundle export/import`); FFI exposes the same engine for in-process orchestration. `import_bundle` keeps its signature and `Result<(), DdbError>` result and follows the per-payload ref namespace contract in [Bundle Protocol](bundle.md#recovery-workflow-and-interface-conformance): an import that fails after fetching (no `master`, a merge failure, or a node-registration or index-rebuild failure) keeps its fetched refs for a retry, while a failure before the fetch (unreadable archive, checksum mismatch, refused namespace, lease timeout) creates none and a failed cleanup after a landed import may leave only some; there is no abort call. A bundle without `master` fails instead of merging another import's kept `master`, and success deletes only its own namespace. Merge failures surface as `DdbError::Io` with a `sync: bundle merge failed:` message (PRD 00177 collapsed the old `DdbError::Git` shape into the policy-derived `Io` variant). `ffi::tests::bundle_import_ffi_*` run the same golden workflow as the CLI e2e scenarios through the public Rust driver.
- **Remote sync (push/pull/fetch)** — bundle-shaped only; no Git remote push/pull/fetch method on `DoogatDriver`. Continuous remote sync runs through CLI `ddb sync` or the GraphQL `sync` mutation.
- **Warnings (structured)** — create, update, search, and reindex carry warnings; the other return types do not yet. See the deprecated entry above; tracked as `Specialized` in the promise matrix until `ffi-typed-errors-v1` adds the channel to the rest.
- **Create/update facade exclusion (PRD 00149) — closed by PRD 00177** — PRD 00149 thinned the create and update workflows onto the app-command facade (`DoogatService::create` / `DoogatService::update`, each returning `AppOutput<ParsedDoogat>`) for GraphQL, CLI, and REST, and deliberately left FFI out. PRD 00177 closed that gap: `create_structured` and `update_structured` build a `CreateCommand` / `UpdateCommand` and call the same facade entrypoints, so FFI shares the contract's business policy, its policy-derived error codes, and its warning channel. The raw-markdown `create_doogat` / `update_doogat` paths that stayed outside the facade are gone.

## Binding Generation

Uses UniFFI proc-macro approach (`uniffi::setup_scaffolding!()` in `lib.rs`). No UDL-based code generation; `src/ddb.udl` is kept as interface documentation.

Generate bindings via the bundled `uniffi-bindgen` binary:

```bash
# Build the cdylib first
cargo build -p ddb-core

# Generate Swift
cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate \
  --library target/debug/libddb_core.dylib \
  --language swift --out-dir out/swift

# Generate Kotlin
cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate \
  --library target/debug/libddb_core.dylib \
  --language kotlin --out-dir out/kotlin
```

Output files:
- Swift: `ddb_core.swift`, `ddb_coreFFI.h`, `ddb_coreFFI.modulemap`
- Kotlin: `uniffi/ddb_core/ddb_core.kt`

Regenerate the bindings after any record or method change. PRD 00176 added
`search_filtered` and `FieldEntry` and widened `SearchResult`, so Swift/Kotlin
consumers built from older bindings do not see the new method or the four new
hit fields until they regenerate.

## Thread Safety

`DoogatDriver` holds a single `svc: Mutex<DoogatService>`. Every exported method acquires that one lock through `with_service` / `with_service_mut`, so all git, SQLite, and sync operations are serialized behind it — there is no multi-lock ordering to reason about. If a prior call panicked while holding the lock the mutex is poisoned, and the helpers map `PoisonError` to `DdbError::Io { msg: "service lock poisoned: ..." }` rather than letting the panic cross the FFI boundary.

During a transaction, `execute_sql` runs against the `SqlEngine` owned by the locked `DoogatService`; the SAVEPOINT lives on the service's SQLite connection and persists across `execute_sql` calls until `commit_transaction` / `rollback_transaction`.

## On-Device Verification

### Prerequisites

#### Swift / iOS / macOS

1. **Xcode** — full install from App Store (Command Line Tools alone are not enough):
   ```bash
   sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
   sudo xcodebuild -license accept
   xcodebuild -runFirstLaunch
   ```
2. **Rust cross-compile targets**:
   ```bash
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin
   ```

#### Kotlin / Android

1. **JDK** (required by `kotlinc` and `jar`):
   ```bash
   brew install openjdk
   ```
2. **Kotlin compiler**:
   ```bash
   brew install kotlin
   ```
3. **cargo-ndk** (Cargo wrapper for Android NDK builds):
   ```bash
   cargo install cargo-ndk
   ```
4. **Android NDK**:
   ```bash
   brew install --cask android-ndk
   export ANDROID_NDK_HOME=/opt/homebrew/share/android-ndk
   ```
   Add the export to your shell profile (`~/.zshrc` or `~/.bashrc`).
   Alternatively, install via Android Studio SDK Manager and set `ANDROID_NDK_HOME=$HOME/Library/Android/sdk/ndk/<version>`.
5. **Rust cross-compile targets**:
   ```bash
   rustup target add aarch64-linux-android x86_64-linux-android
   ```

#### Verify setup

```bash
# Swift side
xcodebuild -version          # should show Xcode version
rustup target list --installed | grep ios  # should list 3 iOS targets

# Android side
java -version                 # should show JDK version
kotlinc -version              # should show Kotlin version
cargo ndk --version           # should show cargo-ndk version
echo $ANDROID_NDK_HOME        # should be set
rustup target list --installed | grep android  # should list 2 targets
```

### Build

```bash
# XCFramework (iOS + macOS)
dev/bin/build-xcframework

# Android AAR
dev/bin/build-android
```

Both scripts use the `vendored` feature to compile OpenSSL and libgit2 from source for cross-compilation targets.

### Test Results

#### Swift on macOS (2026-03-09)

- **Platform**: macOS 26.2, Apple Silicon (arm64), Xcode 26.3, Swift 6.2
- **XCFramework slices**: ios-arm64, ios-arm64_x86_64-simulator, macos-arm64
- **Tests**: 10/10 passed
  - `testCreateAndReadDoogat` — create doogat via FFI, reindex, read back, verify content
  - `testSearch` — create doogat, reindex, FTS5 search by title
  - `testListDoogats` — create doogat, verify it appears in listing
  - `testPerformanceMetrics` — cold start, create, search, reindex latency at 100 doogats
  - `testExecuteSqlReturnsStructuredResult` — execute SQL via SqlEngine, verify structured row/column result
  - `testTransactionCommitAndRollback` — begin/commit/rollback lifecycle, verify atomicity
  - `testListTypeSchemas` — create typedef, list schemas, verify columns and metadata
  - `testMultiTableTypedScenario` — 4-table PRD scenario: workspace/section/link/section-link, joined read, transactional update, type metadata bootstrap
  - `testDeltaBundleExportImport` — register remote node with known_heads, export delta bundle, import into fresh repo, verify only post-sync content
  - `testBundleExportImport` — export full bundle, import into fresh repo, verify round-trip
- **Note**: Tests use `DoogatDriver.createRepo()` and `registerNode()` directly (no CLI binary needed), making them compatible with iOS simulator targets. Verified on macOS slice of the XCFramework.

#### Kotlin on JVM (2026-03-09)

- **Platform**: macOS, JDK 25, Kotlin 2.3.10, JNA 5.16.0
- **Native lib**: `libddb_core.dylib` (release build, host platform)
- **Tests**: 10/10 passed
  - `testCreateAndReadDoogat` — create doogat via FFI, reindex, read back, verify content
  - `testSearch` — create doogat, reindex, FTS5 search by title
  - `testListDoogats` — create doogat, verify it appears in listing
  - `testPerformanceMetrics` — cold start, create, search, reindex latency at 100 doogats
  - `testExecuteSqlReturnsStructuredResult` — execute SQL via SqlEngine, verify structured row/column result
  - `testTransactionCommitAndRollback` — begin/commit/rollback lifecycle, verify atomicity
  - `testListTypeSchemas` — create typedef, list schemas, verify columns and metadata
  - `testMultiTableTypedScenario` — 4-table PRD scenario: workspace/section/link/section-link, joined read, transactional update, type metadata bootstrap
  - `testDeltaBundleExportImport` — register remote node with known_heads, export delta bundle, import into fresh repo, verify only post-sync content
  - `testBundleExportImport` — export full bundle, import into fresh repo, verify round-trip
- **Note**: Tests run on JVM host (not Android emulator). The native library and FFI bindings are verified via JNA on the host platform.
