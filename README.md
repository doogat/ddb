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
cargo test                            # default workspace test selection
cargo test-ci                         # fast local tier (unit/bin targets only)
cargo test-full                       # full cargo suite (Tier 2, CI)
cargo clippy --workspace --all-targets # lint
cargo test -p ddb-e2e                  # Rust smoke and integration scenarios (Tier 2)
cargo test -p ddb-e2e smoke_           # CLI smoke scenarios (Tier 2)
```

Run `cargo build`, all-target Clippy, and `cargo test-ci` locally after each task; CI runs the heavy Tier 2 battery. See [AGENTS.md](AGENTS.md) for the deletion safety exception.

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
dev/bin/build-xcframework  # iOS/macOS XCFramework
dev/bin/build-android      # Android .aar
```

Both scripts check their prerequisites up front and name anything missing. Install the toolchains below once; after that both builds run unattended.

#### iOS/macOS toolchain (`build-xcframework`)

1. **Rust cross-compilation targets** (the host `aarch64-apple-darwin` target ships with rustup):

   ```bash
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
   ```

2. **Full Xcode.** The Command Line Tools are not enough: `xcodebuild -create-xcframework` only ships with the full app (a 15+ GB download). Install Xcode from the App Store (or `mas install 497799835`), then point the developer tools at it and accept the license:

   ```bash
   sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
   sudo xcodebuild -license accept
   xcodebuild -runFirstLaunch
   ```

3. **Verify:**

   ```bash
   xcodebuild -version              # prints "Xcode <n>", not a CLT error
   rustup target list --installed   # lists all three ios targets
   ```

#### Android toolchain (`build-android`)

1. **Rust cross-compilation targets:**

   ```bash
   rustup target add aarch64-linux-android x86_64-linux-android
   ```

2. **Android NDK.** Easiest via Homebrew; the cask installs to `$(brew --prefix)/share/android-ndk`:

   ```bash
   brew install --cask android-ndk
   export ANDROID_NDK_HOME="$(brew --prefix)/share/android-ndk"   # add to your shell profile
   ```

   Alternative: install an NDK through Android Studio's SDK Manager and point at the versioned directory instead: `export ANDROID_NDK_HOME=$HOME/Library/Android/sdk/ndk/<version>`.

3. **cargo-ndk** (drives cargo with the NDK toolchains; reads `ANDROID_NDK_HOME`):

   ```bash
   cargo install cargo-ndk
   ```

4. **Kotlin compiler + JDK.** `kotlinc` compiles the generated bindings into `classes.jar`, and the `jar` packaging tool comes from a JDK:

   ```bash
   mise use -g java@temurin-21 kotlin@latest   # mise-managed (shims java, jar, kotlinc)
   ```

   Or via Homebrew: `brew install --cask temurin && brew install kotlin` (the temurin cask registers with `/usr/libexec/java_home`, which makes the stock `/usr/bin/jar` work).

5. **Verify:**

   ```bash
   ls "$ANDROID_NDK_HOME"             # NDK contents, not "No such file"
   command -v cargo-ndk kotlinc jar   # all three resolve
   rustup target list --installed     # lists both android targets
   ```

#### Prove the whole chain

```bash
dev/bin/build-xcframework   # -> out/swift/DoogatDB.xcframework
dev/bin/build-android       # -> out/kotlin/doogatdb.aar
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
