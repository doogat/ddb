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

`DoogatDriver` wraps `GitRepo` and `Index` behind `Mutex` for thread safety. All methods take `&self` (shared reference via `Arc` on the foreign side).

## Mobile Integration Model

`DoogatDriver` is the embedded runtime boundary for native apps — not a mobile IPC mechanism. On mobile, the recommended model is one host app embedding a single `DoogatDriver` instance, not multiple separately installed apps communicating over `localhost`. Mobile OS sandboxing and background execution limits make inter-app backends non-portable.

Widgets and extensions (iOS WidgetKit, Android AppWidgetProvider) access the same shared repository via App Group storage (iOS) or app-private storage (Android). Whether extensions get their own read-only `DoogatDriver` instance or consume snapshots exported by the host app depends on the coordination strategy — see the [Building Apps guide](../guide/building-apps.md#mobile-mini-apps) for architecture details.

## Promise Boundaries

> **Stability note**: The **Experimental** label at the top of this page refers to API surface stability - method signatures and record shapes may change before a stable release. It does not mean CRUD operations are unreliable. Behavioral promises are tracked independently.

The embedded interface makes the following promises, consistent with the product's 120% parity posture:

| Capability | Promise | Notes |
|-----------|---------|-------|
| Create / read / update / delete (base CRUD methods or typed SQL via `execute_sql`) | `Guaranteed` | Same Git-backed semantics as CLI and GraphQL |
| FTS5 search (`search`, `search_paginated`) | `Guaranteed` | Same `Index` path as the server |
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
| `create_doogat(content, message)` | `parser::parse` → `repo.commit_file` → `index.index_doogat` |
| `read_doogat(id)` | `index.resolve_path` → `repo.read_file` |
| `update_doogat(id, content, message)` | `index.resolve_path` → `repo.commit_file` → `index.index_doogat` |
| `delete_doogat(id, message)` | `index.resolve_path` → `repo.delete_file` → `index.remove_doogat` |

### Query

| Method | Delegates to |
|--------|-------------|
| `search(query)` | `search_paginated(query, MAX, 0)`, returns hits only |
| `search_paginated(query, limit, offset)` | `index.search_paginated` (FTS5 with LIMIT/OFFSET) |
| `list_doogats()` | `repo.list_doogats` |
| `execute_sql(sql)` | See [SQL (SqlEngine-backed)](#sql-sqlengine-backed) |

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
| `register_node(name)` | `sync_manager::register_node`, returns UUID |
| `compact()` | `SyncManager::open` → `compaction::compact` |
| `export_full_bundle(output_path)` | `bundle::export_full_bundle`, returns path |
| `export_delta_bundle(target_node_uuid, output_path)` | `bundle::export_bundle`, returns path |
| `import_bundle(bundle_path)` | `bundle::import_bundle` |

## Error Mapping

`DdbError` is a UniFFI-exported enum mirroring `DoogatError` variants. Each variant carries a `msg: String`. The `From<DoogatError>` impl maps internal errors to FFI-safe variants:

| DoogatError | DdbError |
|------------|---------|
| `Git(msg)` | `Git { msg }` |
| `Yaml(msg)` | `Yaml { msg }` |
| `Sql(msg)` | `Sql { msg }` |
| `Io(e)` | `Io { msg: e.to_string() }` |
| `Toml(msg)` | `Config { msg }` |
| `VersionMismatch { repo, driver }` | `VersionMismatch { msg: "..." }` |

### Service lock access

`DoogatDriver` holds its `DoogatService` behind a `Mutex`. Every FFI method
reaches the service through the central helpers `with_service` and
`with_service_mut`, never `lock().unwrap()`. If an earlier call panicked while
holding the lock the mutex is poisoned; the helpers map `PoisonError` to
`DdbError::Io { msg: "service lock poisoned: ..." }` instead of letting the
panic cross the FFI boundary. Callers therefore always receive a typed error
after a poisoned lock, never a Rust panic.

## FFI Records

- `SearchResult` — `{ id, title, path, snippet, rank }` (mirrors `types::SearchResult`)
- `PaginatedSearchResult` — `{ hits: Vec<SearchResult>, total_count: u64 }`
- `RebuildReport` — `{ indexed, tables_materialized, types_inferred }` (subset of `types::RebuildReport`, omits warnings)
- `AttachmentInfo` — `{ name, mime, size }` (mirrors `types::AttachmentInfo`)
- `SqlResultRecord` — `{ columns, rows, affected_rows, message }` (flat conversion from `SqlResult` enum)
- `TypeSchemaRecord` — `{ table_name, columns, crdt_strategy, template_sections }`
- `ColumnDefRecord` — `{ name, data_type, references, required }`

## Compatibility and Deprecation

Promise labels (`Guaranteed`, `Specialized`, `Intentionally absent`, `Deprecated`) are defined in [Compatibility and Deprecation](../guide/building-apps.md#compatibility-and-deprecation) in the building-apps guide. Every deprecated behavior below names a replacement; entries flagged "Status: planned, not yet implemented" reference candidate follow-up PRD slugs that have not shipped yet.

### Deprecated behavior

- **`DoogatDriver.search` wraps engine errors as `DdbError::Sql(msg)`**: throws the SQL variant carrying engine error text in the string payload; no per-code typed variant, so Swift/Kotlin consumers substring-match `msg` to branch on failure kind. Replacement: candidate `ffi-typed-errors-v1` follow-up PRD will expose AppError codes as typed FFI enum variants on `DoogatError`, mirroring the AppError envelope PRD 00147 shipped for the network transports. Status: planned, not yet implemented.
- **`DoogatDriver.get` throws `DdbError::Io(msg)` on not-found**: error message carries the substring `"not found"`; consumers detect missing-id failures via `msg.contains("not found")`. Replacement: candidate `ffi-typed-errors-v1` follow-up PRD adds `DdbError::NotFound { id }` as a typed variant, mirroring AppError's `NOT_FOUND` code shipped by PRD 00147. Status: planned, not yet implemented.
- **`DoogatDriver.delete` followed by `get` returns the same untyped not-found shape**: the post-delete read inherits the `DdbError::Io(msg)` "not found" shape; consumers cannot distinguish delete-then-get from any other IO failure without substring matching. Replacement: same typed `NotFound` variant under candidate `ffi-typed-errors-v1`. Status: planned, not yet implemented.
- **`DoogatDriver.create` invalid-type error embeds code in `DdbError::Sql(msg)`**: throws the SQL variant whose message contains `TYPE_NOT_REGISTERED`; the structured code AppError already carries internally is recoverable only by parsing the message string. Replacement: candidate `ffi-typed-errors-v1` follow-up PRD exposes AppError validation codes (including `TYPE_NOT_REGISTERED`) as typed FFI enum variants. Status: planned, not yet implemented.
- **FFI return types omit the structured warning channel**: `RebuildReport` explicitly omits warnings and other FFI return types do not carry a structured warning channel; AppWarning entries are dropped at the UniFFI boundary. Replacement: candidate `ffi-typed-errors-v1` follow-up PRD adds a structured-warnings surface to FFI return types (e.g. `AppOutput<T>`-equivalent UniFFI record carrying `warnings: Vec<WarningEntry>`), mirroring the AppOutput shape PRD 00147 established for the network transports. Status: planned, not yet implemented.

### Specialized capabilities

Not deprecated, still binding promises:

- **Attachments** — Attachments feature is Experimental per the stability tier table; methods exist and work, but the capability promise is narrower than core CRUD until Attachments moves Stable.
- **Bundle export/import** — CLI is the canonical bundle workflow (`ddb bundle export/import`); FFI exposes the same engine for in-process orchestration.
- **Remote sync (push/pull/fetch)** — bundle-shaped only; no Git remote push/pull/fetch method on `DoogatDriver`. Continuous remote sync runs through CLI `ddb sync` or the GraphQL `sync` mutation.
- **Warnings (structured)** — see the deprecated entry above; tracked as `Specialized` in the promise matrix until `ffi-typed-errors-v1` adds the structured-warnings channel.
- **Create/update facade exclusion (PRD 00149)** — PRD 00149 thinned the create and update workflows onto the app-command facade (`DoogatService::create` / `DoogatService::update`, each returning `AppOutput<ParsedDoogat>`) for GraphQL, CLI, and REST. FFI was deliberately excluded: `DoogatDriver.create_doogat` and `update_doogat` keep their direct `_raw`/service-method paths with untyped string errors and no structured warning channel. See the deprecated entries above; migration is tracked under `ffi-typed-errors-v1`.

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

## Thread Safety

`DoogatDriver` fields are wrapped in `Mutex`:
- `repo: Mutex<GitRepo>` — serializes all git operations
- `index: Mutex<Index>` — serializes all SQLite operations
- `txn: Mutex<Option<TransactionBuffer>>` — holds buffered writes/deletes during an active transaction

When multiple locks are needed, the canonical acquisition order is **index → repo → txn**. Methods that only need one lock at a time (e.g. `read_doogat`) may drop and reacquire in any order.

During a transaction, `execute_sql` injects the stored `TransactionBuffer` into a fresh `SqlEngine`, executes, then extracts the buffer back. The SAVEPOINT lives on `Index.conn` and persists across calls. This avoids self-referential lifetime issues while maintaining SqlEngine's transaction semantics.

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
