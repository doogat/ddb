# FFI Bindings

> **Experimental**: UniFFI bindings are experimental and may change in future releases. Do not depend on API stability.

**Source**: `ddb-core/src/ffi.rs`

UniFFI-based foreign function interface exposing Doogat DB to Swift and Kotlin via a high-level `DoogatDriver` facade.

## Architecture

```text
Swift/Kotlin app
      │
      ▼
DoogatDriver (ffi.rs)       ← UniFFI proc-macro boundary
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

## FFI Records

- `SearchResult` — `{ id, title, path, snippet, rank }` (mirrors `types::SearchResult`)
- `PaginatedSearchResult` — `{ hits: Vec<SearchResult>, total_count: u64 }`
- `RebuildReport` — `{ indexed, tables_materialized, types_inferred }` (subset of `types::RebuildReport`, omits warnings)
- `AttachmentInfo` — `{ name, mime, size }` (mirrors `types::AttachmentInfo`)
- `SqlResultRecord` — `{ columns, rows, affected_rows, message }` (flat conversion from `SqlResult` enum)
- `TypeSchemaRecord` — `{ table_name, columns, crdt_strategy, template_sections }`
- `ColumnDefRecord` — `{ name, data_type, references, required }`

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
- **Tests**: 9/9 passed
  - `testCreateAndReadDoogat` — create doogat via FFI, reindex, read back, verify content
  - `testSearch` — create doogat, reindex, FTS5 search by title
  - `testListDoogats` — create doogat, verify it appears in listing
  - `testPerformanceMetrics` — cold start, create, search, reindex latency at 100 doogats
  - `testExecuteSqlReturnsStructuredResult` — execute SQL via SqlEngine, verify structured row/column result
  - `testTransactionCommitAndRollback` — begin/commit/rollback lifecycle, verify atomicity
  - `testListTypeSchemas` — create typedef, list schemas, verify columns and metadata
  - `testMultiTableTypedScenario` — 4-table PRD scenario: workspace/section/link/section-link, joined read, transactional update, type metadata bootstrap
  - `testBundleExportImport` — export full bundle, import into fresh repo, verify round-trip
- **Note**: Tests use `DoogatDriver.createRepo()` and `registerNode()` directly (no CLI binary needed), making them compatible with iOS simulator targets. Verified on macOS slice of the XCFramework.

#### Kotlin on JVM (2026-03-09)

- **Platform**: macOS, JDK 25, Kotlin 2.3.10, JNA 5.16.0
- **Native lib**: `libddb_core.dylib` (release build, host platform)
- **Tests**: 9/9 passed
  - `testCreateAndReadDoogat` — create doogat via FFI, reindex, read back, verify content
  - `testSearch` — create doogat, reindex, FTS5 search by title
  - `testListDoogats` — create doogat, verify it appears in listing
  - `testPerformanceMetrics` — cold start, create, search, reindex latency at 100 doogats
  - `testExecuteSqlReturnsStructuredResult` — execute SQL via SqlEngine, verify structured row/column result
  - `testTransactionCommitAndRollback` — begin/commit/rollback lifecycle, verify atomicity
  - `testListTypeSchemas` — create typedef, list schemas, verify columns and metadata
  - `testMultiTableTypedScenario` — 4-table PRD scenario: workspace/section/link/section-link, joined read, transactional update, type metadata bootstrap
  - `testBundleExportImport` — export full bundle, import into fresh repo, verify round-trip
- **Note**: Tests run on JVM host (not Android emulator). The native library and FFI bindings are verified via JNA on the host platform.
