# Doogat DB

Doogat DB is a database engine that pairs decentralized Git-backed storage with conflict-free sync and flexible multi-protocol data access.

## Installation

Download the latest release for your platform from [GitHub Releases](https://github.com/doogat/ddb/releases/latest).

### macOS / Linux

```bash
# set your target: aarch64-apple-darwin, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu
TARGET=aarch64-apple-darwin
TAG=$(curl -fsSL -o /dev/null -w '%{url_effective}' https://github.com/doogat/ddb/releases/latest | xargs basename)
curl -fsSL "https://github.com/doogat/ddb/releases/download/${TAG}/ddb-${TAG}-${TARGET}.tar.gz" | tar xz
sudo mv "ddb-${TAG}-${TARGET}/ddb" /usr/local/bin/
```

### Windows

Download the `ddb-*-x86_64-pc-windows-msvc.zip` asset from [Releases](https://github.com/doogat/ddb/releases/latest), extract, and add the folder to your `PATH`.

### From source

```bash
cargo install --path ddb-cli
```

Or use the release helper:

```bash
dev/bin/release local
```

### Verify

```bash
ddb --version
```

## Updates

`ddb` checks for new releases in the background. When a new version is available, you'll see a notice on the next run.

To update immediately:

```bash
ddb update-bin
```

To roll back to the previous version:

```bash
ddb update-bin --rollback
```

## Stability

### Stable (v0.2 API contract)

- CLI: init, create, read, update, delete, search, query, rename, type, sync, status, compact, reindex, fix, discover, register-node, node, maintenance, sequence
- Git storage format (doogat Markdown, frontmatter schema)
- SQLite FTS5 search
- SQL SELECT, CREATE TABLE, INSERT, UPDATE, DELETE (including bulk, transactions, upsert)
- Multi-device sync (push, pull, merge, CRDT conflict resolution)
- GraphQL server (`ddb serve`) with dynamic schema, subscriptions, and batch mutations
- REST API and PgWire protocol
- `ddb-core` public Rust API for the above

### Experimental

- NoSQL API (`get`, `scan`, `backlinks`)
- UniFFI bindings (Swift/Kotlin)
- Bundle export/import
- Attachments
- Auto-update (`update-bin`)

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- C compiler + pkg-config (for `git2`, `openssl` native deps)
- Optional: `psql` for PgWire integration tests

### Build

```bash
cargo build                # debug build (default dev crates)
cargo build --workspace    # full workspace build
cargo build --release      # release build (default dev crates)
```

### Test

```bash
cargo test                 # fast local tier
cargo test-ci              # bounded CI matrix tier (unit/bin targets only)
cargo test-full            # full cargo suite (includes ddb-e2e)
cargo clippy --workspace   # lint
./tests/smoke.sh             # CLI smoke test
./tests/integration.sh       # full integration (runs smoke first)
```

### Benchmarks

```bash
cargo bench                # run criterion benchmarks
```

### Install locally

If you installed `ddb` from a GitHub release binary (to `/usr/local/bin/`), use `release local` to test your working tree changes. It installs to `~/.cargo/bin/ddb` which takes PATH priority over `/usr/local/bin/`.

```bash
dev/bin/release local      # cargo install from source
ddb --version              # 0.2.0+dev.g<sha> confirms the local build
```

The `+dev.g<sha>` suffix only appears in local builds. CI and tagged releases produce a clean version (e.g. `0.2.0`).

### Release

```bash
dev/bin/release --dry-run patch   # preview version bump
dev/bin/release patch             # bump patch, tag, push
dev/bin/release minor             # bump minor
dev/bin/release major             # bump major
dev/bin/release --pre rc.1 minor  # pre-release: v0.2.0-rc.1
```

### Platform packaging (UniFFI bindings)

```bash
dev/bin/build-xcframework  # iOS/macOS XCFramework (requires Xcode)
dev/bin/build-android      # Android .aar (requires NDK, cargo-ndk, kotlinc)
```

## Documentation

### Book (architecture, technical design, user guide)

Requires [mdbook](https://rust-lang.github.io/mdBook/guide/installation.html):

```bash
cd docs && mdbook serve --open
```

Builds to `docs/book/`.

### API Reference (rustdoc)

```bash
cargo doc --no-deps --open
```

Builds to `target/doc/`.
