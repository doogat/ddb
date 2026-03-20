# ZettelDB

Hybrid Git-CRDT decentralized Zettelkasten database. Git is source of truth; SQLite index is derived/rebuildable.

## Stack

- Rust 2021 edition, workspace with three crates
- Git storage via `git2`, CRDT via `automerge`
- SQLite index via `rusqlite` (FTS5), SQL parsing via `sqlparser`
- CLI via `clap` (binary: `zdb`)
- GraphQL server via `axum` + `async-graphql` (dynamic schema)
- FFI via `uniffi` (proc-macro approach, generates Swift/Kotlin bindings)

## Structure

```
zdb-core/src/       Library crate
  parser.rs         Three-zone Markdown parsing (frontmatter/body/references)
  git_ops.rs        Git repository CRUD, merge, remote sync
  crdt_resolver.rs  Automerge CRDT conflict resolution
  indexer.rs        SQLite FTS5 index, type inference, materialization
  service.rs        Unified orchestration layer (ZettelService) for CLI/FFI/server
  sql_engine.rs     SQL DDL/DML translation (tables as zettel types)
  bundled_types.rs  Built-in type templates (project, contact)
  sync_manager.rs   Multi-device sync orchestration
  compaction.rs     CRDT temp cleanup and git gc
  hlc.rs            Hybrid Logical Clock for causal ordering
  traits.rs         Core trait abstractions (ZettelSource, ZettelStore, etc.)
  ffi.rs            UniFFI ZettelDriver facade for Swift/Kotlin bindings
  types.rs          Shared data structures (ZettelId, ParsedZettel, ZettelMeta)
  error.rs          Error types and Result alias
  zdb.udl           UniFFI interface definition (documentation reference)
zdb-core/benches/   Criterion benchmarks
  crud.rs           CRUD operations at 1K zettels
  search.rs         FTS5 search, SQL SELECT, reindex at 1K zettels
zdb-uniffi-bindgen/ UniFFI bindgen binary (isolated from zdb-core)
zdb-cli/src/        Binary crate (single main.rs)
zdb-server/src/     GraphQL server crate
  lib.rs            Server entrypoint (axum router, actor spawn)
  actor.rs          Thread-safe core bridge (mpsc + oneshot)
  schema.rs         Dynamic GraphQL schema from _typedef zettels
  auth.rs           Bearer token generation + middleware
  config.rs         Server config (~/.config/zetteldb/)
  error.rs          ZettelError → GraphQL error mapping
tests/e2e/          E2E tests (assert_cmd, exercises zdb binary)
tests/smoke.sh      CLI smoke test (init, CRUD, search, SQL, sync, compact)
tests/fixtures/     Test fixtures
dev/bin/             Developer scripts
  release              Version bump, tag, push
  build-xcframework    iOS/macOS XCFramework from UniFFI bindings
  build-android        Android .aar from UniFFI bindings
docs/src/           mdbook documentation (architecture, technical, guide)
```

## Design Principles

Follow SOLID and Clean Architecture principles as adapted for Rust. These are mandatory for all code changes:

- `technical/solid.md` - SOLID principles translated to Rust idioms (traits over inheritance, small focused traits, dependency inversion via generics)
- `technical/clean-architecture.md` - Layer boundaries, dependency direction, I/O at the edges, no panics in library code

## Setup

```
git config core.hooksPath dev/hooks
```

## Conventions

- All modules return `error::Result<T>`
- ZettelId: 14-digit timestamp string (YYYYMMDDHHmmss)
- Zettels stored at `zettelkasten/{id}.md`, typedefs at `zettelkasten/_typedef/{id}.md`
- Data dir: `.zdb/`, node file: `.git/zdb-node`, git signature: `zdb`
- Plan documents go in `.local/plans/` (gitignored), NOT in `docs/`
- Git worktrees go in `.local/worktrees/` (gitignored), nowhere else

## Definition of Done

A task is NOT complete unless ALL of these pass:

1. **Tests** — unit tests in the module AND integration/e2e tests in `tests/` (not just unit tests; use `cargo test --workspace` for the full cargo suite)
2. **Smoke test** — if the change adds a CLI command, server endpoint, or user-facing behavior, add a corresponding scenario to BOTH `tests/smoke.sh` (bash, runs on Linux/macOS) and `tests/smoke.ps1` (PowerShell, runs on Windows) following each file's existing numbered-section + `pass` helper pattern
3. **Docs** — update relevant files in `docs/src/` to reflect any behavioral or API changes
4. **Build** — `cargo clippy --workspace`, fast-tier `cargo test`, and full-suite `cargo test --workspace` all pass
5. **Walkthrough** — if the task adds a CLI command, server endpoint, or user-facing behavior, create an executable showboat walkthrough in `.local/walkthroughs/` (see Showboat Walkthroughs below)
6. **Architecture doc** — if the task changes module boundaries, data flow, or key types, update `docs/src/technical/walkthrough.md`

## Showboat Walkthroughs

Executable feature demos built with [showboat](https://github.com/simonw/showboat), a CLI tool by Simon Willison. Each walkthrough is a Markdown file containing commentary and code blocks with real captured output. `showboat verify` re-runs all code blocks and confirms outputs still match.

### Location and naming

`.local/walkthroughs/{5-digit}-{slug}.md` — e.g. `00001-crud-basics.md`

### Installation

Run via uvx (no install needed):

    uvx showboat --help

Or install persistently: `uv tool install showboat` / `pip install showboat`

### Critical rule: showboat CLI only

Agents **must not** edit walkthrough files directly (no `Edit`, `Write`, `sed`, etc.). All content flows through showboat CLI:

| Command | Purpose |
|---------|---------|
| `showboat init <file> <title>` | Create new walkthrough |
| `showboat note <file> <text>` | Add commentary (also accepts stdin) |
| `showboat exec <file> <lang> <code>` | Run code, capture real output |
| `showboat pop <file>` | Remove last entry (undo failed exec) |
| `showboat image <file> <path>` | Embed an image |
| `showboat verify <file>` | Re-run all blocks, diff against recorded output |
| `showboat extract <file>` | Emit commands to recreate file (for rebuilding) |

Output blocks contain real captured output. Direct file editing defeats the purpose — walkthroughs are proof of work.

### Execution model

Each `showboat exec` runs in its own shell. Variables, background jobs, and working directory do **not** persist between calls. Use `--workdir <dir>` to set the working directory for a command.

`exec` prints captured output to stdout and exits with the same code as the executed command, so agents can react to errors. Use `pop` to remove a failed entry before retrying.

### Patterns

**CLI walkthrough** — use `--workdir` with a fixed temp path (not `mktemp`, since the path must be reused across exec calls):

    WD=/tmp/zdb-demo-feature
    showboat init .local/walkthroughs/00001-feature.md "Feature Name"
    showboat note .local/walkthroughs/00001-feature.md "Initialize a repo."
    showboat exec --workdir $WD .local/walkthroughs/00001-feature.md bash "mkdir -p $WD && zdb init"
    showboat exec --workdir $WD .local/walkthroughs/00001-feature.md bash "zdb create --title 'Test'"
    showboat exec .local/walkthroughs/00001-feature.md bash "rm -rf $WD"

**Server walkthrough** — use PID file pattern since background jobs don't persist across exec calls:

    showboat exec --workdir $WD ... bash "zdb serve --port 19201 --pg-port 19202 & echo \$! > /tmp/zdb-serve.pid"
    showboat exec ... bash "sleep 1 && curl -s http://127.0.0.1:19201/graphql -H 'Content-Type: application/json' -d '{...}'"
    showboat exec ... bash "kill \$(cat /tmp/zdb-serve.pid) && rm /tmp/zdb-serve.pid"

### Maintenance

Walkthroughs are local working documents (`.local/` is gitignored). They can be regenerated anytime. When CLI output changes cause `showboat verify` to fail, regenerate using `showboat extract` to get the original commands, then re-execute.

## Gotchas

- E2E tests require the `zdb` binary — run `cargo build -p zdb-cli` before `cargo test -p zdb-e2e`
- `head_oid()` returns `CommitHash` (a String newtype, access inner via `.0`), not `git2::Oid`
- `merge_frontmatter` is called from both `resolve_conflicts` and `resolve_append_log`

## Commands

```
cargo build                           Build default workspace members (fast local loop)
cargo build --workspace               Build all crates
cargo test                            Run fast local test tier
cargo test-ci                         Run bounded CI matrix tier (unit/bin targets only)
cargo test-full                       Run full cargo test suite (workspace + e2e)
cargo test -p zdb-e2e                 Run e2e tests only
cargo bench                           Run criterion benchmarks (CRUD + search)
cargo bench --no-run                  Compile benchmarks without running
cargo build -p zdb-core --features profiling   Build with tracing instrumentation
SMOKE_PROFILE=quick ./tests/smoke.sh  Quick CLI smoke test
./tests/smoke.sh                      Full CLI + server + sync smoke test
cargo test -p zdb-core --test property_tests   Property-based integration tests
cargo test -p zdb-core --test sync_test        Core sync integration tests
cargo clippy --workspace              Lint
cargo doc --no-deps --document-private-items   Generate rustdoc
cd docs && mdbook build               Build documentation
cd docs && mdbook serve               Serve documentation locally

# Performance thresholds (require --release, local only — advisory in CI)
cargo test --release -p zdb-core --test query_thresholds nfr01_
cargo test --release -p zdb-core --test growth_thresholds nfr02_
cargo test --release -p zdb-core --test sync_thresholds nfr03_

# Property tests (thorough run, ~20 min at 5000 cases)
PROPTEST_CASES=5000 cargo test -p zdb-core --test property_tests

# UniFFI binding generation
cargo run -p zdb-uniffi-bindgen --bin uniffi-bindgen -- generate \
  --library target/debug/libzdb_core.dylib \
  --language swift --out-dir out/swift
cargo run -p zdb-uniffi-bindgen --bin uniffi-bindgen -- generate \
  --library target/debug/libzdb_core.dylib \
  --language kotlin --out-dir out/kotlin
```

## Documentation

Read from `docs/src/` before working on related modules:

- `architecture/overview.md` - System design and data flow
- `architecture/modules.md` - Module responsibilities and boundaries
- `architecture/design-decisions.md` - Key architectural choices
- `technical/data-model.md` - Zettel format, frontmatter schema
- `technical/parser.md` - Three-zone Markdown parsing details
- `technical/git-ops.md` - Git storage layer
- `technical/crdt-resolver.md` - Conflict resolution strategy
- `technical/indexer.md` - SQLite index, FTS5, type inference
- `technical/sql-engine.md` - SQL translation layer
- `technical/sync.md` - Multi-device sync protocol
- `technical/server.md` - GraphQL server architecture and API
- `technical/ffi.md` - UniFFI bindings (ZettelDriver facade)
- `technical/errors.md` - Error handling patterns
- `technical/solid.md` - SOLID principles in Rust
- `technical/clean-architecture.md` - Clean Architecture in Rust
